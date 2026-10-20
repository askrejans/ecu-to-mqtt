# ecu-to-mqtt — technical review and design (0.7.0)

This document records the review of 0.6.1, the problems found, and the design
of the 0.7.0 rework. It is meant for maintainers and for anyone integrating
with the gateway's outputs. Protocol layouts are in
[ECU_PROTOCOLS.md](ECU_PROTOCOLS.md).

## 1. Summary

0.7.0 turns a polling MQTT bridge into a plug-and-play ECU gateway, built on the
same architecture as gps-to-mqtt 0.7:

* **Inputs:** serial (auto-detected port and profile), TCP serial bridge,
  SocketCAN, CAN gateway, built-in simulator. 15 read-only ECU profiles.
* **Outputs:** MQTT (canonical G86 envelope, Speeduino scalar topics, JSON state,
  Home Assistant discovery), HTTP (health, Prometheus, REST, SSE), a built-in
  web dashboard, the terminal dashboard and a synced NDJSON archive.
* **Performance:** 50 Hz from a USB Speeduino (0.6: ~6 Hz), 7 MB RSS, about 1 %
  of one Apple Silicon core at 50 Hz with the dashboard streaming.
* **Licence:** GPL-3.0-only from 0.7.0.

Existing consumers keep working. The canonical envelope (schema 1) keeps
its fields, and the legacy topics keep their names and formats.

## 2. Review of 0.6.1

Severity: **H** = wrong data, crash, outage or exposure; **M** = degraded
operation; **L** = quality/maintainability.

### 2.1 Security

| # | Sev | Finding | Resolution |
|---|-----|---------|------------|
| S1 | H | `cargo audit`: advisories for `bytes` and `crossbeam-channel`, unmaintained `gumdrop`. `paho-mqtt` built the Paho C library and OpenSSL from source, which put a C TLS stack in the binary and broke cross builds (Windows needed `_WIN32_WINNT` and a `libUser32.a` symlink). | Lockfile refreshed; `paho-mqtt` → `rumqttc` (pure Rust, rustls); `gumdrop` → `clap`. Remaining: rumqttc 0.25.1 names `webpki::Error` from rustls-webpki 0.102.8. Verification runs on the patched 0.103 line, so the vulnerable code is not executed. Documented exception in `.cargo/audit.toml`, shared with gps-to-mqtt. |
| S2 | M | MQTT password only inline; no CA or client certificate options in practice. | `mqtt_password_file`, custom CA and mutual TLS over rustls. `--check-config` and `/api/v1/config` mask secrets. |
| S3 | L | No way to tell which commands reach the ECU. | Every command is listed in [ECU_PROTOCOLS.md](ECU_PROTOCOLS.md#identification): `Q`, `A`, `a 00 06`. A fake-ECU test fails on any other byte. |

### 2.2 Correctness

| # | Sev | Finding | Resolution |
|---|-----|---------|------------|
| C1 | H | MQTT died permanently: after `max_retry_count` failed publishes the publisher stopped reconnecting and dropped every later message until restart. | rumqttc event loop with its own backoff (≤ 60 s), forever. State and discovery re-sent on every reconnect. |
| C2 | H | Broker down at startup: the service exited (the unit restarted it into the same failure), and an interactive run continued without MQTT for good. | MQTT connects in the background; acquisition, dashboards and the archive run regardless. |
| C3 | H | Wrong packet documentation: 0.6 described 138-byte packets as "current firmware". A real Speeduino 2025.01 sends 130 bytes (`getTSLogEntry()` cases 0–129). | Packet length learned from the first reply; docs, comments and tests use a real 130-byte capture from a CORE4. |
| C4 | M | Byte 32 labelled as spark flags; it is `status2` (launch, rev limiters, boost cut, spark error, idle control, sync). Status bits were published only as raw integers. | Field renamed. All status bytes are decoded by name from firmware 202501.6 `globals.h` (API, JSON state, dashboard). The `SPK` topic keeps its name. |
| C5 | M | Speeduino channels were not range-checked like the other profiles, so an unconnected TPS published garbage on the canonical topic. | One `add()` range table for every profile; implausible values are omitted. |
| C6 | M | Health was not observable outside the TUI: no endpoint, no metrics, nothing for a container or service monitor. | `/health` returns 200 only with a connected link **and** a sample ≤ 2 s old (`ok` / `no_data` / `ecu_down`); `--healthcheck` for containers. |
| C7 | L | Canonical values carried f32 widening noise (`14.699999809265137`). | Rounded to three decimals at the single `add()` choke point. |

### 2.3 Stability and performance

| # | Sev | Finding | Resolution |
|---|-----|---------|------------|
| P1 | H | Fixed 150 ms sleep after every request ("wait for the ECU to finish"), capping Speeduino at ~6 Hz whatever `refresh_rate_ms` said. | `read_reply()` reads until the expected length arrives (or, while the length is unknown, to a 25 ms idle gap). Measured on USB: 13.7 ms per request-reply, a steady 50 Hz at `refresh_rate_ms = 20`. |
| P2 | H | ~85 legacy topics published on **every** poll (≈ 4 000 messages/s at 50 Hz), unthrottled, through one awaited channel. During an outage the queue filled with stale messages that were flushed on reconnect. | Legacy topics are change-detected, capped at `mqtt_max_rate_hz` (10) and re-sent once per second for late subscribers (not retained: a stale RPM must not outlive the engine). Publishing uses `try_publish` only while connected; drops are counted (`mqtt_messages_dropped_total`). Measured with the simulator: ≈ 130 legacy messages/s instead of ≈ 4 000. |
| P3 | M | The archive did an `fsync` per packet inline in the acquisition path. | Dedicated writer task with a 4096-line queue; batches of up to 256 lines, one write and one `fdatasync` per batch. |
| P4 | M | A split reply (Wi-Fi bridges pause mid-packet) was truncated at the idle gap and desynchronised the stream. Found by the smoke test during the rework. | A known reply length is read to completion within `read_timeout_ms`; regression test with a 180 ms pause. |
| P5 | M | Auto-probing every serial port would cost 2 s each (board reset wait) on hosts with dozens of `ttyS*`. | Only USB adapters are probed; built-in UARTs are named explicitly. |
| P6 | L | Shared state behind a `tokio` RwLock, written from the acquisition loop with an `.await` per update; reconnect logic duplicated for serial, TCP and CAN. | std `RwLock` snapshot, never held across `.await`; one `Backoff` and one event path for all sources. |

## 3. Architecture

```text
 ┌─────────────── sources (one task) ───────────────┐
 │ poller: serial / TCP  (Q identify, A / a 00 06)  │
 │ can_socket (SocketCAN) · can_gateway (TCP JSON)  │──SourceEvent──┐
 │ simulator (real 130-byte packets → same decoder) │               │ mpsc(256), back-pressure
 └──────────────────────────────────────────────────┘               ▼
                                   ┌──────────────── hub ─────────────────┐
                                   │ stamp bootId + sequence → envelope   │──► archive task (batched fdatasync)
                                   │ snapshot: channels, link, rate,      │
                                   │ latency, firmware, Speeduino detail  │
                                   └──────┬──────────────────┬────────────┘
                         broadcast bus    │ (lossy)          │ snapshot (std RwLock)
              ┌───────────────┬───────────┼──────────┬───────┴──────┬──────────┐
              ▼               ▼           ▼          ▼              ▼          ▼
            MQTT          HTTP SSE    Prometheus  HTTP REST    web dashboard   TUI
```

**Back-pressure policy.** Sources → hub is bounded and back-pressured: the hub
does no I/O. Hub → outputs is a lossy broadcast. A slow output lags and skips
(counted) without slowing acquisition or the other outputs. The archive must
not silently lose data, so it has its own bounded queue and counts what it
cannot persist (`archive_errors_total`).

**Samples.** A source produces a `Sample` (source name, `partial`, capture time,
channels, optional full Speeduino packet). The hub assigns the sequence and
serialises the canonical envelope once (`Arc<str>`). MQTT, SSE and the archive
all send those same bytes, so the archive and the broker can never disagree.
A non-partial sample replaces the channel set; a partial one (CAN groups) merges.
Channels expire after 2 s everywhere: snapshot, dashboards and consumers.

**Polling.** Request, read to the expected length, decode, wait for the next
`refresh_rate_ms` tick (`MissedTickBehavior::Delay`, so a slow ECU is never
flooded). Input is cleared before each request so a late reply cannot be
taken for the next one. `max_retry_count` consecutive failures reopen the
link, with exponential backoff between attempts.

**Auto-detection.** `ecu_protocol = "auto"` resolves from the `Q` signature.
`port_name = "auto"` ranks ports (ECU product names 100, known USB-UART vendors
80, other USB 50), opens each USB candidate, waits 2 s for the board reset,
and takes the first one whose signature maps to a profile.

### Modules

| Path | Responsibility |
|------|----------------|
| `src/protocol/` | Profile registry, range table, MegaSquirt 112-byte decoder, `speeduino.rs` (packet, canonical channels, status bits, legacy topics, encoder for tests/simulator), `can_profiles.rs` |
| `src/source/` | `poller` (identify, poll loop), `link` (serial/TCP transport, port ranking), `can_socket`, `can_gateway`, `can_input`, `simulator` |
| `src/hub.rs` | Single consumer of source events; sequencing, snapshot, fan-out |
| `src/sample.rs` | `Sample`, `Stamped`, canonical envelope |
| `src/shared.rs` | Snapshot, counters, health |
| `src/output/` | MQTT, HTTP (+ `web.rs` embedded assets) |
| `src/archive.rs` | Synced NDJSON archive |
| `src/tui/` | ratatui dashboard |
| `src/config.rs` | Flat config, env mapping, validation |
| `web/` | Dashboard (HTML/CSS/JS, no build step; embedded with `include_str!`) |

## 4. Output contracts

### 4.1 MQTT

All topics are under `mqtt_base_topic` (trailing `/` removed).

* **`<base>/status`**: `online` / `offline`, retained, QoS 1; `offline` is the last will.
* **`canonical`**: `<base>/telemetry`, never retained, never throttled. Exactly
  seven keys: `schema` (1), `source`, `partial`, `bootId`, `timestampMs`,
  `sequence`, `channels`. This is the contract of the G86 app
  (`lib/services/ecu/ecu_telemetry.dart`): unknown channels are ignored, a
  sequence or `bootId` change is detected, and channels expire after 2 s.
  New channels may be added; existing names, units and meanings are frozen.
* **`legacy`** (Speeduino only): `<base>/<CODE>` scalar strings, names and number
  formats unchanged from 0.6, published on change (at most
  `mqtt_max_rate_hz`) and re-sent every second, not retained. The G86 web
  dashboard reads `RPM MAP TPS BAT ADV AFT CAD MAT DWL ENG O2P PW1 VE1 WEC`.
* **`json`**: `<base>/state`, retained, ≤ `mqtt_max_rate_hz`:
  `timestampMs, sequence, source, protocol, firmware, channels` (fresh only),
  plus `speeduino` (full packet) and `flags` (active bits as `group.name`).
* **`homeassistant`**: retained discovery documents
  `<prefix>/sensor/<node>/<channel>/config` reading `<base>/state` with
  `<base>/status` as availability and `expire_after: 10`. `<node>` is derived
  from the base topic and is stable across restarts.

### 4.2 HTTP

| Route | Notes |
|-------|-------|
| `GET /health` | 200 `{"status":"ok",…}` with a connected link and a sample ≤ 2 s old; 503 `no_data` / `ecu_down` otherwise |
| `GET /metrics` | Prometheus text 0.0.4: `ecu_connected`, `ecu_samples_total`, `ecu_decode_errors_total`, `ecu_timeouts_total`, `ecu_reconnects_total`, `ecu_sample_rate_hz`, `ecu_sample_age_seconds`, `ecu_poll_latency_ms`, `ecu_channel{name}`, `mqtt_connected`, `mqtt_messages_published_total`, `mqtt_messages_dropped_total`, `archive_errors_total` |
| `GET /` | Web dashboard (when `http_ui = true`, the default) |
| `GET /api/v1/status` | Build info, health, counters |
| `GET /api/v1/live` | Channels with ages, Speeduino detail, every status bit |
| `GET /api/v1/channels` | Channel catalogue: name, label, unit, group, gauge range |
| `GET /api/v1/config` | Effective configuration, secrets masked |
| `GET /api/v1/stream` | SSE: one `sample` event per non-partial sample (≤ 25/s), every partial sample; keep-alive |

`/api/*` requires `Authorization: Bearer <http_token>` when a token is set
(constant-time compare). The default bind is `127.0.0.1`, and binding publicly
without a token logs a warning. Responses carry `nosniff`, `no-store` and
`frame-ancestors 'none'`. CORS is sent only for `http_cors_origin`.

### 4.3 Web dashboard

`GET /` serves a single-page dashboard embedded in the binary. No build step,
no framework, no remote resources. It uses the public API like any client:
REST plus `api/v1/stream`, read with `fetch()` so the bearer token goes in a
header (EventSource cannot set headers). The token is entered once and kept in
the browser's `localStorage`.

| Mode | Content |
|------|---------|
| Dash | Status lamps from the firmware bits (running, sync, warm-up, ASE, DFCO, limiters, launch, boost cut, spark error, knock, fan, protection); gauges with warning zones |
| Engine | Canonical channels with value, unit and age; every Speeduino packet field, grouped |
| Charts | Rolling 30 s / 60 s / 2 min canvas charts, pause, selectable channels (history kept client-side) |
| Diagnostics | Every status bit by group with severity; link, counters with rates, controller |
| Configuration | Wiring, endpoints, effective configuration (read-only, masked) |

Persistent readout: RPM (loops/s, sync losses), throttle, MAP and boost, AFR
and target, coolant and intake, battery, data rate. Lamps: ECU, engine, MQTT,
live stream. Light and dark themes are first-class. The default follows
`prefers-color-scheme`, and the choice is stored. Charts read their colours
from CSS variables, so both themes stay legible. Keys `1`–`5` switch modes;
`°C/°F` is a toggle.

Security: assets are public, data is not (the token applies). The CSP is
`default-src 'none'` with `'self'` for scripts, styles, images and connections,
plus `frame-ancestors 'none'`. All dynamic text uses `textContent`. A test
rejects `innerHTML`, inline scripts and remote URLs in the assets.
Configuration is read-only by design. Changing ECU or broker settings from a
browser would be a remote-control surface, and the ECU is never written.

## 5. ECU support

Profiles, firmware boundaries and channel lists are in
[ECU_PROTOCOLS.md](ECU_PROTOCOLS.md). Hardware evidence:

| Controller | Link | Result |
|------------|------|--------|
| DIY-EFI CORE4, Speeduino 2025.01 (`speeduino 202501`) | USB, CP210x `10c4:ea60`, 115200 | Zero-config detection; 130-byte packets; 50 Hz at `refresh_rate_ms = 20`, 13.7 ms request-reply, 0 timeouts and 0 decode errors over a sustained run; unconnected TPS correctly omitted |
| Same, 0.6.1 | Same | ~6 Hz (fixed 150 ms wait) |

## 6. Security model

* Untrusted input: ECU bytes, TCP bridge and CAN gateway peers, HTTP clients.
  Replies are bounded (512 bytes, 4 KiB gateway lines) and decoded without
  panics; short or malformed packets are counted, not trusted.
* The ECU is never written beyond the read requests above. There is no
  tuning, burn, flash or CAN transmit code path.
* The HTTP API exposes engine data. It binds to loopback by default, and `/api/*`
  accepts a bearer token. Put a TLS reverse proxy in front for remote access.
* MQTT: TLS with built-in or custom roots, mTLS, password files.
* systemd unit: dedicated user, `ProtectSystem=strict`, empty capability set,
  syscall filter, private state directory (0700) for the archive (files
  0600). Docker: non-root, health check, read-only root filesystem and dropped
  capabilities in the compose example.

## 7. Verification

* `cargo test`: 126 tests, all passing. Coverage includes every decoder (real
  130-byte capture, scale/sign, boundaries, status bits), range table, CAN
  frames (both ID widths, stale rejection, fragmented gateway input), the
  poller against a fake TCP ECU that fails on any non-read byte, split replies,
  auto-detect resolution, config/env mapping and examples, MQTT payloads
  (envelope keys, legacy change detection and refresh, state, HA), HTTP routes, auth and SSE, the
  dashboard's CSP and asset rules, the archive, and the TUI in a test backend.
* `scripts/smoke_mqtt_bridge.py`: all 15 profiles through the compiled binary
  to a real Mosquitto broker and subscriber, with archive cross-check.
* `cargo clippy --all-targets -- -D warnings` and `cargo fmt`: clean.
  `cargo audit`: clean with the documented exception.
* Live: the CORE4 over USB (detection, 50 Hz, every web mode in both themes);
  the simulator against Mosquitto with all four MQTT formats (canonical 50 Hz,
  state and legacy ≈ 10 Hz, 12 HA sensors, `online` / `offline`).
* Not exercised on hardware: MegaSquirt and CAN ECUs (published layouts,
  protocol tests and the smoke test only), Windows, TLS/mTLS brokers, the G86
  Flutter app against this build.

## 8. Migrating from 0.6

| 0.6 | 0.7 |
|-----|-----|
| `port_name` default `/dev/ttyACM0`, `ecu_protocol` default `speeduino` | Defaults `auto` / `auto`. Explicit values behave as before. |
| Legacy topics on every poll | On change (≤ `mqtt_max_rate_hz`, 10) plus a 1 s refresh. |
| `expected_data_length = 138` | Harmless: the length is learned from the first reply. |
| `max_retry_count` = attempts before giving up | Consecutive failed polls before the link is reopened; reconnection never gives up. |
| MQTT client: paho/OpenSSL | rumqttc/rustls. `mqtt_ca_cert_path` and client cert/key keep their names. |
| No HTTP | `http_enabled = true` for the API and dashboard (Docker image: on, loopback). |
| Licence MIT | GPL-3.0-only |

## 9. Known limitations

* MegaSquirt serial uses the 112-byte compatibility reply (no CRC, a fixed
  channel subset). Full `A` packets depend on the firmware INI and are not decoded.
* CAN profiles cannot be auto-detected from a passive bus.
* The legacy scalar topics exist only for Speeduino.
* Opening a serial port resets Arduino-based boards (DTR). The gateway waits
  2 s, but the ECU itself restarts. Hardware without auto-reset is unaffected.
* Auto-detection probes USB adapters only; built-in UARTs need `port_name`.
