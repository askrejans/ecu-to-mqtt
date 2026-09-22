# ECU protocols — version 0.4.0

This version adds read-only telemetry from MegaSquirt and six additional ECU
families to the existing Speeduino bridge. The project, executable, service,
environment prefix and default MQTT topic retain their existing names.

Select the **broadcast protocol configured in the ECU**, not just its brand.
CAN layouts are manufacturer-specific. A CAN socket is not an automatic decoder
for every ECU, and none of these profiles writes maps, calibration or control
commands. Sensor availability depends on ECU firmware, wiring and configuration.

## Supported profiles

| `ecu_protocol` | ECU / required output | Connection and default CAN base | Decoded channels |
|---|---|---|---|
| `speeduino` (default) | Existing Speeduino primary `A` output layout | Serial or raw serial-over-TCP; usually 115200 baud | Existing legacy topics plus canonical RPM, TPS, MAP, coolant, intake, battery, ignition and AFR |
| `ms2` | MS2/Extra 3.3+ compatibility output | Serial or raw serial-over-TCP | RPM, TPS, MAP, coolant, intake, battery, ignition, AFR |
| `ms3` | MS3 1.2+ compatibility output | Serial or raw serial-over-TCP | Same compatibility subset |
| `ms3_pro` | MS3Pro with the MS3 compatibility command | Serial or raw serial-over-TCP | Same compatibility subset |
| `microsquirt` | MicroSquirt running compatible MS2/Extra firmware | Serial or raw serial-over-TCP | Same compatibility subset |
| `megasquirt_can_dash` | MegaSquirt dash broadcasting enabled | CAN gateway; 11-bit, normally 500 kbit/s; `1512` (`0x5E8`) | RPM, TPS, MAP, coolant, intake, battery, ignition, AFR |
| `megasquirt_can_realtime` | MegaSquirt realtime broadcasting, groups 0–3 enabled | CAN gateway; 11-bit, normally 500 kbit/s; `1520` (`0x5F0`) | Same channels, using the different realtime layout |
| `haltech_can_v2` | Haltech documented V2 stream, including compatible Elite / Nexus configurations | CAN gateway; 11-bit, 1 Mbit/s; `0x360` | RPM, TPS, MAP, fuel/oil/brake pressure, ignition, lambda 1/2, speed, gear, battery, coolant/intake/fuel temperatures, lateral/longitudinal G |
| `maxxecu_can_v12` | MaxxECU default CAN 1.2 | CAN gateway; 11-bit, 500 kbit/s; `0x520` | RPM, TPS, MAP, lambda, ignition, speed, battery, intake, coolant, gear |
| `maxxecu_can_v13` | MaxxECU default CAN 1.3 | CAN gateway; 11-bit, 500 kbit/s; `0x520` | 1.2 channels plus oil/fuel pressure, oil temperature, brake/clutch switches and G |
| `ecumaster_emu_can` | EMU PRO / EMU Black compatible EMU CAN stream | CAN gateway; 11-bit; `0x600`; match the configured ECU port bitrate | RPM, TPS, MAP, intake/coolant/oil temperatures, speed, oil/fuel pressure, ignition, lambda, gear, battery |
| `aemnet_can` | AEMnet v150609 ECU stream (Infinity with matching output) | CAN gateway; **29-bit extended**, 500 kbit/s; `0x01F0A000` | RPM, TPS, intake/coolant/oil temperatures, lambda 1/2, speed, gear, ignition, battery, MAP, fuel/oil pressure |
| `link_generic_dash` | Link / Vi-PEC **Generic Dash** stream | CAN gateway; 11-bit, configured ECU bitrate and ID; default ID `1000` | RPM, MAP, TPS, coolant/intake/oil temperatures, battery, gear, ignition, lambda 1/2, fuel/oil pressure, four wheel speeds |
| `link_generic_dash2` | Link **Generic Dash 2 / Race Technology Dash2Pro** stream | CAN gateway; 11-bit, configured bitrate; default base `1000`, four consecutive IDs | RPM, **gauge boost**, TPS, coolant/intake/oil temperatures, battery, ignition, driven-wheel speed, oil/fuel pressure, lambda 1/2, gear |
| `motec_m1_pdm` | MoTeC M1 GPx 1.4 published PDM output | CAN gateway; 11-bit, configured ECU bitrate; `0x118` / `0x119` | RPM at **100 RPM resolution**, TPS, speed, coolant/oil/fuel/transmission/differential temperatures, brake/clutch switches |

The serial MegaSquirt profiles request the documented `61 00 06` command and
require the complete 112-byte compatibility reply. They do not decode arbitrary
firmware-dependent full `A` packets or perform tuning. This legacy subset has no
CRC; use a reliable wired link or the ECU's supported CAN broadcast where possible.
MS1, arbitrary third-party MicroSquirt firmware, proprietary MoTeC M800/M1 dash
layouts, OEM diagnostic protocols and custom DBCs are not implied by this list.

MaxxECU 1.3 additions require firmware that implements those messages (oil/fuel
additions are documented from 1.135). For EMU Black V3 use firmware 3.047 or later
for the corrected TPS broadcast. EMU PRO CAN1 is 1 Mbit/s; CAN2 is configurable.
Link Generic Dash and Generic Dash 2 have different byte order and scaling and
must not be interchanged. Dash 2's driven-speed field occupies one byte, so its
wire range is 0–255 km/h despite the wider range printed in PCLink's help table.

## Connect a CAN adapter

The Rust process accepts newline-delimited CAN frames over TCP. The included
gateway receives frames using [python-can](https://python-can.readthedocs.io/en/stable/bus.html)
and supports its SocketCAN, PCAN, Kvaser, SLCAN and other installed backends.
Install the adapter's driver, set the correct bitrate, enable the desired ECU
broadcast and provide the required bus termination according to its manual.
The gateway never calls a CAN transmit function. Hardware acknowledgement and
listen-only behavior remain adapter-specific.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r scripts/can-requirements.txt
# Linux SocketCAN example; configure can0's bitrate before starting it.
.venv/bin/python scripts/can_gateway.py --interface socketcan --channel can0 --bitrate 1000000
# In a second terminal:
cargo run -- --config examples/haltech-can.toml
```

For Windows, the virtualenv interpreter is `.venv\Scripts\python.exe`; an example
adapter selection is `--interface pcan --channel PCAN_USBBUS1 --bitrate 500000`.
For a supported macOS SLCAN adapter use `--interface slcan --channel
/dev/tty.usbserial-ADAPTER --bitrate 500000`. Backend availability depends on the
adapter and OS. These adapter combinations have not been hardware-tested here.

Use the same CAN gateway with any CAN profile by changing `ecu_protocol` and,
only if the ECU output is configured differently, `can_base_id`. TOML permits
hexadecimal IDs; `SPEEDUINO_CAN_BASE_ID` environment overrides use decimal.
The default gateway listens on **127.0.0.1:29536**. It has no authentication or
encryption; keep it on loopback or carry it through an authenticated tunnel.
Broker TLS is configured separately in the Rust bridge.

The gateway sends records like this, with a current Unix capture time in ms:

```json
{"id":864,"extended":false,"timestampMs":1750000000000,"data":[23,112,3,245,2,238,0,0]}
```

That illustrative timestamp must be replaced with a current capture time for a
test. Exactly eight data bytes are accepted. Extended vs standard identifiers
must match the profile. Error, remote and CAN FD frames are excluded. Lines are
bounded to 4096 bytes; malformed, stale (>2 seconds), future (>2 seconds), wrong
format and irrelevant frames do not produce fresh telemetry. Keep ECU bridge,
gateway and consuming phone clocks synchronized. A custom gateway may omit
`timestampMs`, in which case receipt time is used; this cannot identify stale
data buffered by that custom gateway.

## MQTT and G86

Every profile publishes canonical JSON on `<mqtt_base_topic>/telemetry`.
Trailing slashes are normalized. Speeduino also retains its existing legacy
scalar topics; the new profiles publish the canonical topic only.

```json
{"schema":1,"source":"haltech-can-v2","bootId":"bridge-start-id","sequence":42,"timestampMs":1750000000000,"partial":true,"channels":{"rpm":6000,"manifoldKpa":101.3,"throttle":75.0}}
```

`bootId` changes on process restart. `sequence` increases within that process.
`partial: true` means the packet updates **only** its listed channels. A coolant
packet never refreshes an old RPM reading. Consumers expire individual channels
after two seconds and reject duplicate or out-of-order samples. Messages are not
retained. A full MQTT queue drops new canonical samples instead of blocking ECU
acquisition; queued samples retain their original timestamps.

Channel names carry their units: RPM, percentages, °C, kPa, volts, degrees BTDC,
km/h and G. `manifoldKpa` is absolute, while `boostKpa` is gauge pressure. Oil,
fuel and brake pressure are gauge values where specified by the manufacturer.
Lambda is kept as lambda; no fuel-dependent AFR conversion is assumed.
`ecuSpeedKmh` and `wheelSpeed*` do not overwrite phone GPS speed. A brake switch
is a binary signal and is not presented as measured pedal percentage. Unsupported
or invalid readings are omitted, not replaced with zeros.

In G86 Pro, save the broker hostname, port, credentials and **ECU topic prefix**
to match this configuration. For `examples/haltech-can.toml`, use
`/g86/car-1/ecu`. Use `mqtts://` and port 8883 for a remote TLS broker. Each car
needs a distinct topic and broker ACL; do not publish several cars to one prefix.
Credentials can be supplied using `SPEEDUINO_MQTT_USERNAME` and
`SPEEDUINO_MQTT_PASSWORD` rather than committed configuration files.

The existing interactive terminal dashboard remains Speeduino-specific.
Other profiles use the MQTT path and structured logs; `log_level = "debug"`
shows decoded channel counts. `mqtt_enabled = false` decodes without publishing.

## Primary protocol references

The implementation uses the following manufacturer specifications. Tables are
not redistributed in this repository.

- [MegaSquirt serial protocol, 2014-10-28](https://www.msextra.com/doc/pdf/Megasquirt_Serial_Protocol-2014-10-28.pdf): 112-byte compatibility command, offsets and units.
- [MegaSquirt CAN broadcasting](https://www.msextra.com/doc/pdf/Megasquirt_CAN_Broadcast.pdf): dash and realtime base IDs and groups.
- [Speeduino interface protocol](https://wiki.speeduino.com/en/reference/Interface_Protocol): existing primary command. The legacy parser is retained; verify its packet layout against the firmware installed on your Speeduino.
- [Haltech CAN protocol library](https://support.haltech.com/portal/en/kb/haltech/technical-library/haltech-can-protocol-information): V2 specification, Kelvin temperatures, gauge-pressure offsets, signed gear and G units.
- [MaxxECU default CAN protocol](https://www.maxxecu.com/webhelp/can-default_maxxecu_protocol.html): 1.2 / 1.3 fields and firmware requirements.
- [ECUMaster EMU PRO manual](https://www.ecumaster.com/files/EMU_PRO/EMU_PRO_Manual.pdf), CAN stream table; [EMU Black V3 changes](https://www.ecumaster.com/files/EMU_BLACK_V3/changeLogV3.pdf), TPS correction.
- [AEMnet v150609](https://documents.aemelectronics.com/techlibrary_ta2_infinity_aemnet_configuration.pdf): extended-ID Infinity stream and scaling.
- [Link PCLink official download](https://linkecu.com/software-support/pc-link-downloads/): PCLink 7.8.2 English Help, **CAN → Device Specific CAN Information → Generic Dash / Generic Dash 2**, including compound groups, endianness and scale factors.
- [MoTeC M1 to PDM messaging](https://assets.motec.com.au/strapi/M1_To_PDM_CAN_Messaging_fd06806969.pdf): GPx 1.4 PDM messages. This is the limited published PDM stream, not universal MoTeC telemetry.

## Verification and remaining bench work

Run `cargo test`, `cargo clippy --all-targets` and
`python3 -m unittest discover -s scripts -p 'test_can_gateway.py'`.
After `cargo build`, install Mosquitto's broker and client CLI tools and run
`python3 scripts/smoke_mqtt_bridge.py`. This starts a temporary loopback-only
broker and exercises all 15 profiles through the compiled process to an actual
MQTT subscriber. It stops its processes and removes its temporary configs on exit.
Tests cover scale/sign conversion, frame boundaries, both identifier widths,
fragmented TCP input, stale frame rejection, serial command/reply forwarding and
canonical MQTT envelopes. Test vectors are derived from the published layouts,
not captures from each physical ECU. Physical controller/adapter testing,
firmware-specific availability and on-car comparison with the manufacturer's
software remain required before claiming hardware certification.

Local verification on 22 September 2026: 90 Rust tests and two gateway tests
passed; the compiled bridge passed the local broker smoke test. Clippy completed
with existing style warnings in legacy configuration, parser, MQTT and TUI code;
it is not a warning-free build. No packages have been uploaded for this version.
