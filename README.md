# ecu-to-mqtt

Plug-and-play, read-only engine telemetry gateway. Connect a Speeduino or
MegaSquirt over USB, a serial-to-network bridge or a CAN bus, and publish
decoded, timestamped channels to MQTT, a REST / SSE API with a built-in web
dashboard, and Prometheus. Written in Rust; runs on a Raspberry Pi, in Docker,
as a systemd service or interactively with a terminal dashboard.

Nothing is ever written to the ECU: no tuning, no map access, no burns, no CAN
transmit. Serial ECUs receive only the documented read requests (`Q`, `A`,
`a 00 06`).

## Highlights

- **Zero configuration for USB ECUs.** `port_name = "auto"` and
  `ecu_protocol = "auto"` find the controller by its signature and pick the
  profile. Re-plugging and ECU resets are handled.
- **Fast.** The poller requests the next packet as soon as the previous reply
  is complete. Measured on a Speeduino 2025.01 over USB: a steady 50 Hz at a
  20 ms refresh, 14 ms per request-reply, no timeouts.
- **15 ECU profiles.** Speeduino, MegaSquirt MS2/MS3/MS3Pro/MicroSquirt, and
  documented CAN broadcasts from MegaSquirt, Haltech, MaxxECU, ECUMaster,
  AEMnet, Link and MoTeC.
- **Industry-standard outputs.** MQTT: the canonical G86 envelope, Speeduino
  scalar topics, a JSON state document and Home Assistant discovery. Also
  HTTP REST / SSE and Prometheus.
- **Web dashboard** with Dash, Engine, Charts, Diagnostics and Configuration
  modes, in light and dark themes.
- **Resilient.** Slow or offline outputs never block acquisition. MQTT
  reconnects by itself, including when the broker is down at startup.
  Health reflects real data freshness.
- **Built-in simulator** for demos and development without an ECU.

## Quick start

```bash
cargo build --release
./target/release/ecu-to-mqtt
```

On a terminal this opens the dashboard and looks for an ECU on the USB serial
ports. Without a config file it publishes to an MQTT broker on
`localhost:1883`.

```bash
# Dashboard + web UI against the real ECU, no broker needed
ECU_TO_MQTT_MQTT_ENABLED=false ECU_TO_MQTT_HTTP_ENABLED=true ecu-to-mqtt
# then open http://127.0.0.1:9091

# Which port is my ECU on? (--probe sends only the read-only `Q`)
ecu-to-mqtt --list-ports --probe

# Validate a config without starting
ecu-to-mqtt --check-config -c settings.toml

# No hardware: built-in Speeduino simulator
ecu-to-mqtt --simulate
```

## Installation

Pre-built packages, no Rust toolchain needed.

**Debian / Ubuntu**

```bash
curl -fsSL https://g86racing.com/packages/gpg.key | sudo gpg --dearmor -o /usr/share/keyrings/g86racing-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/g86racing-archive-keyring.gpg] https://g86racing.com/packages/apt stable main" | sudo tee /etc/apt/sources.list.d/g86racing.list
sudo apt update && sudo apt install ecu-to-mqtt
```

**Fedora / RHEL / Rocky**

```bash
sudo tee /etc/yum.repos.d/g86racing.repo <<'EOF'
[g86racing]
name=G86Racing packages
baseurl=https://g86racing.com/packages/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://g86racing.com/packages/gpg.key
EOF
sudo dnf install ecu-to-mqtt
```

**macOS (Homebrew)**

```bash
brew tap askrejans/g86racing
brew trust askrejans/g86racing        # Homebrew asks once for third-party taps
brew install ecu-to-mqtt
```

**Windows**: download the `.zip` from <https://g86racing.com/packages/windows/>,
copy `settings.toml.example` to `settings.toml`, and run
`ecu-to-mqtt.exe --config settings.toml`. To run as a service use
[NSSM](https://nssm.cc) (`nssm install ecu-to-mqtt …`).

**After a Linux package install**

```bash
sudo cp /etc/ecu-to-mqtt/settings.toml.example /etc/ecu-to-mqtt/settings.toml
sudo $EDITOR /etc/ecu-to-mqtt/settings.toml
sudo systemctl restart ecu-to-mqtt
journalctl -u ecu-to-mqtt -f
```

Packages install the binary, a hardened systemd unit, the example config and
the licence. They also create an `ecu-to-mqtt` service user in `dialout` and
`tty`.

## Supported ECUs

| `ecu_protocol` | ECU / required broadcast | Transport | Example config |
|---|---|---|---|
| `auto` (default) | Speeduino or MegaSquirt, from the `Q` signature | serial / TCP | [speeduino.toml](examples/speeduino.toml) |
| `speeduino` | Speeduino primary `A` output | serial / TCP | [speeduino.toml](examples/speeduino.toml), [speeduino-tcp.toml](examples/speeduino-tcp.toml) |
| `ms2` | MegaSquirt MS2/Extra 3.3+ | serial / TCP | [megasquirt-ms2-serial.toml](examples/megasquirt-ms2-serial.toml) |
| `ms3` | MegaSquirt MS3 1.2+ | serial / TCP | [megasquirt-ms3-serial.toml](examples/megasquirt-ms3-serial.toml) |
| `ms3_pro` | MS3Pro (MS3 compatibility command) | serial / TCP | [megasquirt-ms3pro-serial.toml](examples/megasquirt-ms3pro-serial.toml) |
| `microsquirt` | MicroSquirt on MS2/Extra firmware | serial / TCP | [microsquirt-serial.toml](examples/microsquirt-serial.toml) |
| `megasquirt_can_dash` | MegaSquirt dash broadcast | CAN | [megasquirt-can-dash.toml](examples/megasquirt-can-dash.toml), [socketcan](examples/megasquirt-can-dash-socketcan.toml) |
| `megasquirt_can_realtime` | MegaSquirt realtime broadcast, groups 0–3 | CAN | [megasquirt-can-realtime.toml](examples/megasquirt-can-realtime.toml) |
| `haltech_can_v2` | Haltech V2 (Elite / Nexus) | CAN | [haltech-can-v2.toml](examples/haltech-can-v2.toml), [socketcan](examples/haltech-can-v2-socketcan.toml) |
| `maxxecu_can_v12` | MaxxECU default CAN 1.2 | CAN | [maxxecu-can-v12.toml](examples/maxxecu-can-v12.toml) |
| `maxxecu_can_v13` | MaxxECU default CAN 1.3 | CAN | [maxxecu-can-v13.toml](examples/maxxecu-can-v13.toml) |
| `ecumaster_emu_can` | ECUMaster EMU PRO / EMU Black | CAN | [ecumaster-emu-can.toml](examples/ecumaster-emu-can.toml) |
| `aemnet_can` | AEMnet v150609 (29-bit extended) | CAN | [aemnet-can.toml](examples/aemnet-can.toml) |
| `link_generic_dash` | Link / Vi-PEC Generic Dash | CAN | [link-generic-dash.toml](examples/link-generic-dash.toml) |
| `link_generic_dash2` | Link Generic Dash 2 / Dash2Pro | CAN | [link-generic-dash2.toml](examples/link-generic-dash2.toml) |
| `motec_m1_pdm` | MoTeC M1 GPx 1.4 PDM output | CAN | [motec-m1-pdm.toml](examples/motec-m1-pdm.toml) |

Pick the **broadcast the ECU is configured to send**, not just the brand. Link
Generic Dash and Generic Dash 2 use different scaling, and MegaSquirt's dash
and realtime streams have different layouts. CAN profiles cannot be detected
from a passive bus, so they are always set explicitly.

The Speeduino profile is validated against a real controller (DIY-EFI CORE4,
firmware 2025.01; a capture is in `tests/fixtures/`). The other profiles are
built from published specifications and covered by protocol tests. On-car
validation against each manufacturer's own software is still outstanding.
Firmware boundaries, packet layouts and channel lists are in
[ECU_PROTOCOLS.md](ECU_PROTOCOLS.md).

## Connections

| `connection_type` | Use for | Key settings |
|---|---|---|
| `serial` (default) | USB cable, RS-232, Raspberry Pi UART | `port_name = "auto"` or a path, `baud_rate` |
| `tcp` | Serial-to-network bridge ([io-to-net](https://github.com/askrejans/io-to-net), ESP32, Moxa, USR), or the CAN gateway | `tcp_host`, `tcp_port` |
| `can` | Linux SocketCAN interface (lowest latency, no helper process) | `can_interface` |
| `simulator` | Demos, development, dashboards without a car | none |

`port_name = "auto"` probes USB serial adapters only, ranked by likelihood
(ECU product names first, then common USB-UART bridges). Name built-in UARTs
such as `/dev/serial0` explicitly. Opening a port resets Arduino-based boards,
so the gateway waits 2 s before the first request.

### Native CAN (Linux)

Bring the interface up with the ECU's bitrate, then point the gateway at it.
Interface setup is privileged and deliberately left outside this program.

```bash
sudo ip link set can0 up type can bitrate 1000000   # Haltech V2: 1 Mbit/s; MaxxECU/MegaSquirt usually 500 kbit/s
ecu-to-mqtt --config examples/haltech-can-v2-socketcan.toml

# Virtual bus for testing
sudo modprobe vcan && sudo ip link add dev vcan0 type vcan && sudo ip link set up vcan0
```

Frames carry kernel receive timestamps, so a frame that sat in the socket
queue is recognised as stale instead of being republished as live data.
Error, remote and CAN FD frames are ignored, and the socket is never written
to.

### CAN gateway (any OS)

```bash
python3 -m venv .venv
.venv/bin/python -m pip install -r scripts/can-requirements.txt
.venv/bin/python scripts/can_gateway.py --interface socketcan --channel can0 --bitrate 1000000
ecu-to-mqtt --config examples/haltech-can-v2.toml      # in another terminal
```

The gateway forwards newline-delimited JSON frames on `127.0.0.1:29536` and
supports every python-can backend (SocketCAN, PCAN, Kvaser, SLCAN …). It has
no authentication, so keep it on loopback or inside an authenticated tunnel.

## Outputs

### MQTT

Select any combination with `mqtt_formats` (default `["canonical", "legacy"]`):

| Format | Topics | Notes |
|---|---|---|
| `canonical` | `<base>/telemetry` | Envelope per sample (G86 app contract), never retained, never throttled |
| `legacy` | `<base>/RPM`, `/MAP`, `/CLT`, `/AFR`, `/PW1` … (~90 Speeduino codes) | Sent on change (≤ `mqtt_max_rate_hz`) and re-sent every second, not retained; codes in [ECU_PROTOCOLS.md](ECU_PROTOCOLS.md#speeduino-scalar-topics) |
| `json` | `<base>/state` | Retained document: fresh channels, full Speeduino packet, status bits, firmware; ≤ `mqtt_max_rate_hz` |
| `homeassistant` | `homeassistant/sensor/<node>/…/config` | Discovery for RPM, throttle, MAP, boost, temperatures, AFR, lambda, battery, advance, oil pressure, speed; reads `<base>/state` |

**On demand.** A consumer on a slow link (the G86-ESP-DSP display over
Wi-Fi) need not take the 50 Hz stream. It publishes its own reply topic as the
payload to `<base>/telemetry/get`, and ecu-to-mqtt answers once on that topic
with the latest sample: a canonical envelope of the fresh channels (QoS 0, not
retained). There is no answer while the ECU link is down or the sample is
stale. The consumer asks again after using the answer, so it gets as many
samples as it can handle and never a queue of old ones. Each request is answered with a sample the consumer hasn't had yet: when
there is nothing new, the request is held until the next sample (at most
2 s). A consumer can keep several requests in flight (the G86 displays keep
10) and gets every new sample once, as soon as it exists, without duplicates.
Per reply topic, answers are limited by a token bucket (bursts of 12, 100 a
second on average), for up to 32 reply topics.

`<base>/status` is `online`/`offline` (retained, with a last will). State is
re-sent after every reconnect. While the broker is unreachable live samples
are dropped and counted, and acquisition never waits. TLS: `mqtt_use_tls`,
`mqtt_ca_cert_path`, and `mqtt_client_cert_path` + `mqtt_client_key_path`
for mutual TLS. Use `mqtt_password_file` for Docker/systemd secrets.

Canonical envelope:

```json
{"schema":1,"source":"speeduino-primary-a","partial":false,"bootId":"1585-1792493938610",
 "timestampMs":1792493938611,"sequence":42,
 "channels":{"rpm":3025,"throttle":43,"manifoldKpa":64,"boostKpa":-37,"coolantC":86,
             "intakeC":30,"batteryV":13.9,"afr":14.5,"afrTarget":14.7,"ignitionDeg":26}}
```

- `partial: true` means the sample updates only the listed channels (CAN
  groups). A non-partial sample replaces the channel set.
- `bootId` changes on restart; `sequence` increases within a run.
- Units are in the names: `manifoldKpa` (absolute), `boostKpa` (gauge),
  `coolantC`, `batteryV`, `ignitionDeg`, `lambda`, `ecuSpeedKmh`, `lateralG` …
- Unsupported or implausible readings are omitted, never zero-filled.
  Consumers ignore channels they do not know and expire them after 2 s.

### HTTP API and web dashboard

```toml
http_enabled = true
http_bind    = "127.0.0.1"   # "0.0.0.0" for the network (then set http_token)
http_port    = 9091
http_token   = "change-me"   # protects /api/*
```

| Endpoint | |
|---|---|
| `/` | Web dashboard (`http_ui = false` to disable) |
| `/health` | 200 when the ECU link is up and data is ≤ 2 s old, otherwise 503 (`no_data`, `ecu_down`) |
| `/metrics` | Prometheus: `ecu_connected`, `ecu_samples_total`, `ecu_sample_rate_hz`, `ecu_poll_latency_ms`, `ecu_channel{name}`, error, reconnect and MQTT counters |
| `/api/v1/status` · `/live` · `/channels` · `/config` | JSON snapshots (`/config` is masked and read-only) |
| `/api/v1/stream` | Server-Sent Events, one event per sample (≤ 25/s) |

The dashboard has five modes (keys `1`–`5`):

- **Dash**: status lamps and gauges with warning zones.
- **Engine**: canonical channels with age, plus every field of the ECU packet.
- **Charts**: rolling 30 s / 60 s / 2 min charts of selectable channels.
- **Diagnostics**: every firmware status bit, link, counters and controller details.
- **Configuration**: wiring, endpoints and the effective settings.

It has light and dark themes (default: follows the system), °C/°F, and
works with the token. It uses a strict Content-Security-Policy and loads
nothing from outside the gateway.

### Local archive

`telemetry_log = "/var/lib/ecu-to-mqtt/telemetry.ndjson"` appends every
canonical envelope and syncs it to disk in batches on a dedicated writer,
independent of MQTT. Files are created with mode 0600 and capped at 512 MiB per
active file. Rotate by renaming (logrotate with `nocreate`, no
`copytruncate`). Write failures are counted in `archive_errors_total` and logged
at most once a minute. The archive does not create G86 sessions; record on the
connected phone for timed sessions.

## Configuration

All settings are flat keys in `settings.toml` and can be overridden with
`ECU_TO_MQTT_<KEY>` environment variables (or a `.env` file):

```bash
ECU_TO_MQTT_MQTT_HOST=broker.lan ECU_TO_MQTT_MQTT_FORMATS=canonical,homeassistant ecu-to-mqtt
```

Config files are searched in `./settings.toml`, `./ecu-to-mqtt.toml`, next to
the binary and in `/etc/ecu-to-mqtt/settings.toml`, or given with `-c`.
Every key is documented in [example.settings.toml](example.settings.toml).

| Area | Keys |
|---|---|
| ECU | `ecu_protocol`, `can_base_id`, `refresh_rate_ms`, `read_timeout_ms`, `expected_data_length` |
| Connection | `connection_type`, `port_name`, `baud_rate`, `tcp_host`, `tcp_port`, `can_interface`, `max_retry_count`, `initial_retry_delay_ms`, `max_retry_delay_ms` |
| MQTT | `mqtt_enabled`, `mqtt_host`, `mqtt_port`, `mqtt_base_topic`, `mqtt_formats`, `mqtt_max_rate_hz`, `mqtt_qos`, `mqtt_client_id`, `mqtt_username`, `mqtt_password(_file)`, `mqtt_use_tls`, `mqtt_ca_cert_path`, `mqtt_client_cert_path`, `mqtt_client_key_path`, `mqtt_homeassistant_prefix`, `message_buffer_size` |
| HTTP | `http_enabled`, `http_bind`, `http_port`, `http_token`, `http_cors_origin`, `http_ui` |
| Logging | `log_level`, `log_json`, `telemetry_log` |

`ECU_TO_MQTT_NO_TUI=1` (or `--headless`) forces service/log mode.

Upgrading from 0.6: existing config files keep working. The defaults are now
`port_name = "auto"` and `ecu_protocol = "auto"`, and the HTTP API is new. See
[CHANGELOG.md](CHANGELOG.md).

## Docker

```bash
docker compose up -d        # edit environment in docker-compose.yml first
docker compose logs -f
```

The compose example runs as a non-root user on a read-only root filesystem
with all capabilities dropped. Map the ECU with `devices:` (prefer
`/dev/serial/by-id/…`); native SocketCAN needs `network_mode: host`.
`HEALTHCHECK` uses `ecu-to-mqtt --healthcheck`. Build it yourself with
`docker build -t ecu-to-mqtt .`.

## Running modes

| Invocation | Behaviour |
|---|---|
| Terminal | Interactive dashboard: connection, rate, latency, firmware, every ECU field, logs (`q` quits) |
| systemd / Docker / pipe, or `--headless` | Structured logs (`log_json = true` for JSON) |
| `mqtt_enabled = false` | Display/API only, no broker needed |
| `--simulate` | Built-in Speeduino engine model through the full pipeline |

## CLI

```
ecu-to-mqtt [OPTIONS]
  -c, --config <FILE>   Config file
      --check-config    Validate, print effective settings (secrets masked), exit
      --list-ports      List serial ports ranked by ECU likelihood
      --probe           With --list-ports: ask each port for its signature (sends only `Q`)
      --simulate        Built-in Speeduino simulator instead of a real ECU
      --headless        Never start the terminal dashboard
      --healthcheck     Exit 0 if /health is OK (container probes)
  -V, --version / -h, --help (lists every profile)
```

## Troubleshooting

| Symptom | Check |
|---|---|
| `no USB serial ports found` / no ECU answered | `ecu-to-mqtt --list-ports --probe`; Linux permissions: `sudo usermod -aG dialout $USER` and log in again; close TunerStudio (one program per port) |
| ECU found but samples slow | Over a TCP bridge the bridge's serial side must run at 115200; check `ecu_poll_latency_ms` |
| `/health` returns 503 `no_data` | Link is up but no valid replies: wrong `ecu_protocol`, or a CAN profile with the broadcast disabled |
| Implausible channel missing | By design: out-of-range readings (e.g. an unconnected TPS) are omitted |
| MQTT messages dropped | `mqtt_messages_dropped_total` rises while the broker is down; state is re-sent on reconnect |
| Nothing on MQTT | `mosquitto_sub -h <broker> -t '<base>/#' -v`; `log_level = "debug"` |

## Building packages

`scripts/build_packages.sh` cross-compiles DEB/RPM (x64, arm64), a Windows
ZIP (x64) and macOS tarballs (x64, arm64) (`--platform`, `--arch`, `--type`,
`--no-cross`; see `--help`). It needs `cross` + Docker for Linux/Windows
targets, `dpkg` and `rpm`, and the macOS rustup targets. Output goes to
`release/<version>/`.

## Development

```bash
cargo test                                    # unit, integration and real-ECU fixture tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo audit                                   # policy in .cargo/audit.toml
python3 -m unittest discover -s scripts -p 'test_can_gateway.py'
cargo build && python3 scripts/smoke_mqtt_bridge.py   # every profile → real broker (needs mosquitto)
cargo test -- --ignored                       # SocketCAN tests on vcan0 (Linux)
```

Architecture, contracts, measurements and the security model are documented
in [TECHNICAL.md](TECHNICAL.md). AI-assisted development guidance lives in
[AGENTS.md](AGENTS.md) and [skills/](skills/).

## Related projects

- [gps-to-mqtt](https://github.com/askrejans/gps-to-mqtt): companion GNSS gateway
- [io-to-net](https://github.com/askrejans/io-to-net): serial-to-TCP bridge
- [speeduino-serial-sim](https://github.com/askrejans/speeduino-serial-sim): external ECU simulator
- [G86 Web Dashboard](https://github.com/askrejans/G86-web-dashboard): MQTT telemetry dashboard

## License

GPL-3.0-only. See [LICENSE](LICENSE).
