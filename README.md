# Speeduino-to-MQTT

A Rust application that reads real-time engine data from Speeduino, MegaSquirt and documented Haltech, Link, MaxxECU, ECUMaster, AEMnet and MoTeC CAN streams and publishes it to an MQTT broker. Supports hardware serial ports, TCP/IP serial bridges and a CAN adapter gateway. The interactive terminal dashboard remains available for Speeduino.

**Local version 0.4.0:** see [ECU protocols and setup](ECU_PROTOCOLS.md) for exact
controller/firmware boundaries, supported channels, CAN adapter instructions and
example configurations. These additions have automated protocol tests; physical
ECU validation remains outstanding. Existing installation instructions below do
not imply that 0.4.0 has been published.

The project name is unchanged. [RENAME_PLAN.md](RENAME_PLAN.md) describes a future
rename and compatibility migration; the rename has not been executed.

![speeduino-to-mqtt](https://github.com/user-attachments/assets/769b1ad8-092c-4101-8461-65cdcd04bb9b)

## Features

- **ECU protocol** – issues the [`A` real-time data command](https://wiki.speeduino.com/en/reference/Interface_Protocol), parses all bytes of the response including EMAP, CAN inputs (CN01–CN16), VVT, flex fuel, boost and more.
- **Dual connection modes** – hardware serial (`/dev/ttyACM0`, COM3 …) or raw TCP socket for WiFi/Ethernet–serial bridges (ESP32, Moxa, USR-VIS410, …).
- **Interactive TUI** – when run from a terminal (TTY detected) a live four-panel dashboard is displayed: connection status, ECU gauges, live MQTT stats and a scrolling log.
- **Optional MQTT** – set `mqtt_enabled = false` (or `SPEEDUINO_MQTT_ENABLED=false`) to run in display-only mode with no broker required.
- **Flexible configuration** – TOML config file, environment variables with `SPEEDUINO_` prefix, and automatic `.env` file loading from the working directory.
- **Systemd service** – ships with a ready-made service unit; the `scripts/build_packages.sh` helper builds installable DEB and RPM packages.
- **85+ MQTT topics** – every ECU parameter is published as a short three-letter code under a configurable base topic.

> **Testing:** [speeduino-serial-sim](https://github.com/askrejans/speeduino-serial-sim) can be used to generate synthetic ECU data without a real ECU.

<img width="1344" height="806" alt="Screenshot 2026-03-09 at 21 20 44" src="https://github.com/user-attachments/assets/e4e255c0-e1d1-4e81-91ec-810c47b5e7f7" />

## Running modes

| Invocation | Behaviour |
|---|---|
| Terminal / bench (`ssh`, local shell) | Interactive TUI rendered via `ratatui` |
| `systemd` service / no TTY | Structured text logging to stdout |
| `mqtt_enabled = false` | No broker needed; data shown in TUI only |
| `mqtt_enabled = true` (default) | Data published to MQTT broker |

---

## Installation

Pre-built packages are available for all major platforms — no Rust toolchain needed.

### Debian / Ubuntu

```bash
curl -fsSL https://g86racing.com/packages/apt/gpg.key | sudo gpg --dearmor \
     -o /usr/share/keyrings/speeduino-archive-keyring.gpg

echo "deb [signed-by=/usr/share/keyrings/speeduino-archive-keyring.gpg] \
     https://g86racing.com/packages/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/speeduino.list

sudo apt update
sudo apt install speeduino-to-mqtt
```

### Fedora / RHEL / Rocky Linux

```bash
sudo tee /etc/yum.repos.d/g86racing.repo <<'EOF'
[g86racing]
name=G86Racing packages
baseurl=https://g86racing.com/packages/rpm
enabled=1
gpgcheck=0
EOF

sudo dnf install speeduino-to-mqtt
```

### macOS (Homebrew)

```bash
brew tap askrejans/g86racing
brew install speeduino-to-mqtt
```

To run as a background service (launchd):

```bash
brew services start askrejans/g86racing/speeduino-to-mqtt
```

Config is installed to `$(brew --prefix)/etc/speeduino-to-mqtt/settings.toml.example`.

### Windows

1. Download the latest `.zip` from [https://g86racing.com/packages/windows/](https://g86racing.com/packages/windows/).
2. Extract and copy `settings.toml.example` → `settings.toml`, then edit it.
3. Run interactively: `.\speeduino-to-mqtt.exe --config settings.toml`
4. Install as a Windows Service (optional, using [NSSM](https://nssm.cc)):

```powershell
nssm install speeduino-to-mqtt "C:\speeduino-to-mqtt\speeduino-to-mqtt.exe"
nssm set    speeduino-to-mqtt AppParameters "--config C:\speeduino-to-mqtt\settings.toml"
nssm start  speeduino-to-mqtt
```

### After Linux installation

```bash
sudo cp /etc/speeduino-to-mqtt/settings.toml.example /etc/speeduino-to-mqtt/settings.toml
sudo $EDITOR /etc/speeduino-to-mqtt/settings.toml
sudo systemctl start speeduino-to-mqtt
```

---

## Docker

The easiest way to run on any Linux machine or Raspberry Pi — no Rust toolchain needed.

### Quick start (serial connection)

```bash
# 1 – Clone / download the repo (or just grab docker-compose.yml)
git clone https://github.com/askrejans/speeduino-to-mqtt
cd speeduino-to-mqtt

# 2 – Edit the environment variables in docker-compose.yml
#     (SPEEDUINO_MQTT_HOST, SPEEDUINO_PORT_NAME, etc.)
$EDITOR docker-compose.yml

# 3 – Build and start
docker compose up -d

# Follow logs
docker compose logs -f
```

### Quick start (TCP / Wi-Fi bridge)

Uncomment the `speeduino-to-mqtt-tcp` service in `docker-compose.yml` and set `SPEEDUINO_TCP_HOST` / `SPEEDUINO_TCP_PORT`.

### docker-compose.yml reference

All configuration is done via **environment variables** in `docker-compose.yml` — no config file editing required.

| Variable | Default | Description |
|---|---|---|
| `SPEEDUINO_CONNECTION_TYPE` | `serial` | `serial` or `tcp` |
| `SPEEDUINO_PORT_NAME` | `/dev/ttyACM0` | Serial device inside the container |
| `SPEEDUINO_BAUD_RATE` | `115200` | Serial baud rate |
| `SPEEDUINO_TCP_HOST` | — | TCP bridge hostname / IP |
| `SPEEDUINO_TCP_PORT` | — | TCP bridge port |
| `SPEEDUINO_MQTT_ENABLED` | `true` | Set `false` for display-only |
| `SPEEDUINO_MQTT_HOST` | `localhost` | MQTT broker hostname |
| `SPEEDUINO_MQTT_PORT` | `1883` | MQTT broker port |
| `SPEEDUINO_MQTT_BASE_TOPIC` | `/GOLF86/ECU/` | Base MQTT topic prefix |
| `SPEEDUINO_MQTT_USERNAME` | — | Broker username (optional) |
| `SPEEDUINO_MQTT_PASSWORD` | — | Broker password (optional) |
| `SPEEDUINO_LOG_LEVEL` | `info` | `trace` \| `debug` \| `info` \| `warn` \| `error` |

### Using a settings.toml file instead

Mount your own config file over the default:

```yaml
# docker-compose.yml
volumes:
  - ./settings.toml:/etc/speeduino-to-mqtt/settings.toml:ro
```

Environment variables always take priority over the file, so you can use both.

### Serial port access on Linux

The container user is added to the `dialout` group. Ensure your host user can also access the device:

```bash
sudo usermod -aG dialout $USER   # then log out and back in
```

### Build the image yourself

```bash
docker build -t speeduino-to-mqtt .
```

---

## Building from source

### Quick start

```bash
# 1 – Build
cargo build --release

# 2 – Copy and edit config
cp example.settings.toml settings.toml
$EDITOR settings.toml

# 3 – Run (TUI auto-enabled when attached to a terminal)
./target/release/speeduino-to-mqtt

# 4 – Or pass a custom config path
./target/release/speeduino-to-mqtt --config /etc/speeduino-to-mqtt/settings.toml
```

## CLI options

```
Usage: speeduino-to-mqtt [options]

Options:
  -h, --help          Print help
  -c, --config FILE   Path to TOML config file (default: settings.toml)
```

---

## Configuration

Copy `example.settings.toml` to `settings.toml` and adjust the values. Every setting can also be set via an environment variable with the `SPEEDUINO_` prefix, or in a `.env` file in the working directory.

```toml
# ── Connection ──────────────────────────────────────────────────
connection_type = "serial"   # "serial" | "tcp"

# Serial (used when connection_type = "serial")
port_name  = "/dev/ttyACM0"
baud_rate  = 115200

# TCP bridge (used when connection_type = "tcp")
# tcp_host = "192.168.1.100"
# tcp_port = 23

# ── ECU protocol ────────────────────────────────────────────────
# expected_data_length = 120   # 119–256; 121 enables EMAP
# read_timeout_ms      = 2000
refresh_rate_ms        = 20

# ── MQTT ────────────────────────────────────────────────────────
mqtt_enabled    = true
mqtt_host       = "localhost"
mqtt_port       = 1883
mqtt_base_topic = "/GOLF86/ECU/"
# mqtt_username = ""
# mqtt_password = ""
# mqtt_use_tls  = false
```

Key environment variables:

| Variable | Description |
|---|---|
| `SPEEDUINO_CONNECTION_TYPE` | `serial` or `tcp` |
| `SPEEDUINO_PORT_NAME` | Serial device path |
| `SPEEDUINO_TCP_HOST` / `SPEEDUINO_TCP_PORT` | TCP bridge address |
| `SPEEDUINO_MQTT_ENABLED` | `true` / `false` |
| `SPEEDUINO_MQTT_HOST` / `SPEEDUINO_MQTT_PORT` | Broker address |
| `SPEEDUINO_MQTT_USERNAME` / `SPEEDUINO_MQTT_PASSWORD` | Broker credentials |
| `SPEEDUINO_LOG_LEVEL` | `trace` \| `debug` \| `info` \| `warn` \| `error` |

---

## MQTT topics

All values are published to `<mqtt_base_topic><CODE>`, e.g. `/GOLF86/ECU/RPM`.

### Engine basics
| Code | Description |
|---|---|
| `RPM` | Engine speed (rev/min) |
| `TPS` | Throttle position (0–255 raw) |
| `MAP` | Manifold absolute pressure (kPa) |
| `BAR` | Barometric pressure (kPa) |
| `BAT` | Battery voltage (V, 1 dp) |
| `SCL` | Loop counter (secl) |
| `SYN` | Sync loss counter |

### Temperatures
| Code | Description |
|---|---|
| `IAT` | Intake air temperature (°C) |
| `CLT` | Coolant temperature (°C) |
| `MAT` | IAT raw byte (backward-compatible) |
| `CAD` | Coolant raw byte (backward-compatible) |
| `FTP` | Fuel temperature (°C) |

### O2 / AFR
| Code | Description |
|---|---|
| `O2P` | Primary O2 sensor |
| `O2S` | Secondary O2 sensor |
| `AFT` | AFR target (real units, 1 dp) |

### Fuel & injection
| Code | Description |
|---|---|
| `VE1` / `VE2` / `VEC` | Volumetric efficiency current / table 1 / table 2 |
| `PW1`–`PW4` | Injector pulse width channels 1–4 (ms, 1 dp) |
| `FLD` | Fuel load |
| `FTC` | Fuel temp correction |

### Ignition
| Code | Description |
|---|---|
| `ADV` / `AD1` / `AD2` | Ignition advance (degrees) |
| `DWL` | Dwell time (ms, 1 dp) |
| `SPK` | Spark status bitfield |
| `IGD` | Ignition load |

### Corrections
| Code | Description |
|---|---|
| `COR` | Combined corrections |
| `BTC` | Battery correction |
| `EGC` | EGO (O2) correction |
| `ITC` | IAT correction |
| `WEC` | Warm-up enrichment correction |
| `BRC` | Baro correction |
| `ASE` | After-start enrichment |
| `TAE` | Transient acceleration enrichment (%) |

### Flex fuel / ethanol
| Code | Description |
|---|---|
| `ETH` | Ethanol % |
| `FLC` | Flex fuel correction |
| `FIC` | Flex ignition correction |
| `FBC` | Flex boost correction |

### Boost
| Code | Description |
|---|---|
| `BST` | Boost target (kPa) |
| `BSD` | Boost duty cycle (%) |

### VVT
| Code | Description |
|---|---|
| `VA1` / `VA2` | VVT 1/2 actual angle |
| `VT1` / `VT2` | VVT 1/2 target angle |
| `VD1` / `VD2` | VVT 1/2 duty cycle |

### CAN inputs
| Code | Description |
|---|---|
| `CN01`–`CN16` | CAN input channels 1–16 (u16 each) |

### Miscellaneous
| Code | Description |
|---|---|
| `VSS` | Vehicle speed |
| `GER` | Current gear |
| `FPR` | Fuel pressure |
| `OPR` | Oil pressure |
| `ILL` | Idle load |
| `MPD` | MAP dot (rate of change) |
| `TPD` | TPS dot |
| `TAD` | TPS ADC |
| `CIT` | Closed-loop idle target |
| `WMI` | WMI pulse width |
| `LPS` | Loops per second |
| `FRM` | Free RAM |
| `RPD` | RPM dot |
| `TOF` | Test output flags |
| `NER` | Next error code |
| `STA` / `ENG` / `ST3` / `ST4` | Status bitfields |
| `EPS` | Engine protect status |
| `OUT` | Output status |
| `SDS` | SD card / TunerStudio status |
| `EMP` | EMAP pressure (published only when packet ≥ 121 bytes) |

---

## Building packages

The `scripts/build_packages.sh` script cross-compiles for **all platforms**.

### Targets

| Platform | Arch   | Rust triple                     | Output     |
|----------|--------|---------------------------------|------------|
| Linux    | x64    | x86_64-unknown-linux-gnu        | .deb + .rpm |
| Linux    | arm64  | aarch64-unknown-linux-gnu       | .deb + .rpm |
| Windows  | x64    | x86_64-pc-windows-gnu           | .zip       |
| macOS    | x64    | x86_64-apple-darwin             | .tar.gz    |
| macOS    | arm64  | aarch64-apple-darwin            | .tar.gz    |

### Mac prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# cross – Docker-based cross-compiler for Linux + Windows targets
cargo install cross
# Docker Desktop must be running (and Rosetta enabled for Apple Silicon)

# macOS SDK (already present if Xcode CLT is installed)
xcode-select --install

# macOS cross-arch targets (native cargo, no Docker needed)
rustup target add x86_64-apple-darwin aarch64-apple-darwin

# Apple Silicon: pre-install the cross-compilation toolchains that 'cross'
# needs to mount into Docker. --force-non-host allows installing toolchains
# that can't execute natively on ARM Mac but are needed inside the container.
for TRIPLE in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu; do
    rustup toolchain install stable-$TRIPLE --force-non-host --profile minimal
done

# Apple Silicon Docker prerequisite: enable in Docker Desktop →
#   Settings → General → "Use Rosetta for x86/amd64 emulation on Apple Silicon"

# DEB packaging tool
brew install dpkg

# RPM packaging tool
brew install rpm
```

### Build commands

```bash
# Everything – all platforms, all arches, all package types
./scripts/build_packages.sh

# Linux only (DEB + RPM, all arches)
./scripts/build_packages.sh --platform linux

# Single format / arch
./scripts/build_packages.sh --platform linux  --arch arm64 --type deb
./scripts/build_packages.sh --platform windows --arch x64
./scripts/build_packages.sh --platform mac     --arch arm64

# Use local cargo instead of cross (you must have all toolchains installed)
./scripts/build_packages.sh --no-cross

./scripts/build_packages.sh --help
```

Output layout:

```
release/<version>/
  linux/
    deb/   *.deb (amd64, arm64)
    rpm/   *.rpm (x86_64, aarch64)
  windows/
         *.zip (x64)
  mac/
         *.tar.gz (x86_64, arm64)
         sha256sums.txt
```

Each Linux package:
- Installs the binary to `/usr/bin/speeduino-to-mqtt`
- Installs the systemd unit to `/lib/systemd/system/speeduino-to-mqtt.service`
- Drops an example config at `/etc/speeduino-to-mqtt/settings.toml.example`
- Creates a `speeduino` system user (in `dialout` + `tty` groups for serial access)
- Enables the service on install

---

## Related projects

- [speeduino-serial-sim](https://github.com/askrejans/speeduino-serial-sim) – ECU data simulator for testing
- [GPS-to-MQTT](https://github.com/askrejans/gps-to-mqtt) – companion GPS bridge
- [G86 Web Dashboard](https://github.com/askrejans/G86-web-dashboard) – web dashboard for MQTT telemetry data
