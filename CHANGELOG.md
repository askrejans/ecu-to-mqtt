# Changelog

## 0.7.2

### Added
- On-demand telemetry for consumers on slow links. Publish a reply topic to
  `<base>/telemetry/get` to receive one canonical envelope with the latest
  fresh channels on it (QoS 0, not retained, nothing when the link is down or
  the sample is stale). The consumer asks again after each answer, so it never
  builds a backlog. Answers are rate limited to one per 10 ms per reply topic,
  for up to 32 reply topics. Used by G86-ESP-DSP 0.2.

## 0.7.1

### Fixed
- Speeduino field scaling now follows firmware 202501.6 (`logger.cpp`,
  `sensors.cpp`, `globals.h` and its `speeduino.ini`), verified against a real
  controller:
  - TPS is sent in 0.5 % steps (since firmware 202201): `throttle` and the
    `TPS` topic were twice the real value.
  - Pulse widths (`PW1`–`PW8`) and dwell (`dwell`, `actualDwell`) are
    microseconds: `injectorPwMs`, `dwellMs` and the `PW*`/`DWL`/`ADW` topics
    were 100× too large.
  - Boost duty is a percentage (`BSD` was ×100); VVT angles/targets/duty and
    fan duty are 0.5 steps.
  - TPSdot, MAPdot, fuel/ignition load, flex ignition and flex boost
    corrections are signed. Oil and fuel pressure are PSI.

## 0.7.0

### Contract changes
- Licence changed to GPL-3.0-only.
- Legacy Speeduino scalar topics keep their names and formats but are published
  when a value changes (at most `mqtt_max_rate_hz`, default 10) and re-sent
  once per second, so late subscribers fill in. 0.6 published every topic on
  every poll (~90 messages per packet, ≈ 4 000/s at 50 Hz; now ≈ 130/s).
- Defaults: `port_name = "auto"` and `ecu_protocol = "auto"`. Explicit values
  behave as before.
- The canonical `<base>/telemetry` envelope (schema 1) keeps its fields; the
  Speeduino profile now also sends `partial: false`. New channels: `boostKpa`
  (from the ECU's barometer), `baroKpa`, `afrTarget`, `veCurrent`,
  `injectorPwMs`, `dwellMs`, `syncLossCount`. Speeduino values are range-checked
  like the other profiles, so an unconnected sensor reading out of range is
  omitted. Values are rounded to three decimals.
- `expected_data_length` only sets the first request: the packet length is
  learned from the ECU. Firmware 2025.01 sends 130 bytes (0.6 documented 138).

### Added
- Web dashboard and HTTP API (`http_enabled`): Dash, Engine, Charts,
  Diagnostics and Configuration modes, light/dark themes, °C/°F;
  `/health`, `/metrics` (Prometheus), `/api/v1/{status,live,channels,config,stream}`
  (REST + SSE), bearer token, CORS, strict CSP.
- Auto-detection: `ecu_protocol = "auto"` reads the `Q` signature;
  `port_name = "auto"` probes USB serial adapters ranked by likelihood.
  `--list-ports`, `--probe`.
- MQTT formats (`mqtt_formats`): `canonical`, `legacy`, `json` (`<base>/state`),
  `homeassistant` discovery; `<base>/status` with last will; `mqtt_max_rate_hz`,
  `mqtt_password_file`, CA and client certificates over rustls.
- Speeduino status bits (engine, status1–5, engine protection, air
  conditioning) decoded from the firmware 2025.01 definitions, in the API, the
  `json` state and the dashboards.
- Built-in simulator (`connection_type = "simulator"` or `--simulate`).
- `--headless`, `--healthcheck` (Docker `HEALTHCHECK`), `--check-config` prints
  the effective settings with secrets masked.
- Terminal dashboard shows firmware, sample rate and request latency.

### Fixed
- Polling rate: a fixed 150 ms wait after every request capped Speeduino at
  ~6 Hz. Replies are now read to completion, so a USB Speeduino sustains 50 Hz
  (measured: 14 ms per request-reply).
- MQTT: the publisher stopped permanently after ten failed publishes, and the
  service exited when the broker was down at startup. It now reconnects with
  backoff, and acquisition never waits on the broker.
- A reply split across TCP segments with a pause (Wi-Fi bridges) is reassembled
  instead of rejected.
- The archive writer batches lines and syncs once per batch instead of an
  `fsync` inline per packet.
- Status byte 32 is decoded as `status2` (it was labelled as spark flags).
- Dependency advisories: `bytes`, `crossbeam-channel`; unmaintained `gumdrop`
  replaced by `clap`. No OpenSSL: `paho-mqtt` replaced by `rumqttc` (pure Rust).

### Changed
- Internal restructure: `protocol/` (decoders), `source/` (serial/TCP poller,
  CAN, simulator), `hub` (sequencing, snapshot, archive), `output/` (MQTT,
  HTTP, web). See [TECHNICAL.md](TECHNICAL.md).
- Docker image: runs `--headless` with the HTTP API on loopback and a
  health check; the compose example uses a read-only root filesystem.
- systemd unit runs `--headless` from `/var/lib/ecu-to-mqtt`.
- Packages ship the licence; Linux/Windows builds fail loudly on errors.

## 0.6.1
- Local canonical NDJSON archive (`telemetry_log`), independent of MQTT.

## 0.5.0
- Renamed from `speeduino-to-mqtt`; 15 ECU profiles; native SocketCAN input.
