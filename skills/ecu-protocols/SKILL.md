---
name: ecu-protocols
description: Add or change an ecu-to-mqtt ECU profile, decoder, canonical channel, Speeduino field or status bit, source (serial/TCP/CAN/simulator) or poll behaviour, keeping the ECU strictly read-only.
---

# ECU protocols

## Read-only, always

The gateway writes only `Q` (identify), `A` (Speeduino) and `a 00 06`
(MegaSquirt compatibility reply). It never transmits on CAN. Do not add
tuning, burn, page-read, reset or "test output" commands, even behind a flag,
and do not run them against a connected ECU. `poller.rs` tests use a fake ECU
that panics on any other byte; a new serial profile gets the same test.

## Where things live

| Change | Place |
|--------|-------|
| New profile name | `EcuProtocol` (`protocol/mod.rs`): variant, `ALL`, `name()`, `label()`, `source()`, `command()` or CAN base/offsets; serde name = config value |
| Signature detection | `EcuProtocol::from_signature()` + test in `poller.rs` (`resolve`) |
| CAN decoder | `protocol/can_profiles.rs` `decode()`; group offsets from the manufacturer spec |
| Speeduino field | `protocol/speeduino.rs`: `SpeeduinoData` field with byte offset in the doc comment, `parse()`, `encode()` (tests and simulator), and the TUI/web field lists |
| Status bit | the `ENGINE`/`STATUS*`/`PROTECT`/`AIRCON` tables, from `globals.h` of the firmware release named in the comment |
| Canonical channel | `protocol::add()` range table first (unknown names are dropped), then the decoder. Add it to `CATALOG` in `output/http.rs` for the dashboards |
| Example config | `examples/<profile>.toml`; the config tests load every example |
| Docs | ECU_PROTOCOLS.md table and channel list; README supported-ECU table |

## Rules

- Units in channel names (`coolantC`, `manifoldKpa`, `boostKpa` is gauge).
  Lambda stays lambda; no fuel-dependent AFR conversion.
- Omit unknown, unsupported or implausible values; never publish a zero that
  was not measured (an absent wideband reads 0, so `afr` is omitted).
- A complete packet sets `partial: false` (replaces the channel set); CAN
  groups are `partial: true` (merge). Capture time is taken when the reply
  arrives (`now_ms()` in the poller, kernel timestamps on SocketCAN).
- Speeduino packet length is learned from the first reply; firmware 2025.01
  sends 130 bytes. Never assume a length from the docs alone; check
  `getTSLogEntry()` in the firmware release.
- Poll timing: `read_reply()` reads to the expected length; unknown lengths
  end on a 25 ms idle gap. No fixed sleeps.
- Rate-limit logs in loops (`source::RateLimit`).

## Verify

`cargo test` (decoder vectors, boundaries, fake ECU), then
`cargo build && python3 scripts/smoke_mqtt_bridge.py` (add the profile to its
case lists), then `cargo run -- --simulate` for the dashboards. A real
controller: see the hardware-validation skill.
