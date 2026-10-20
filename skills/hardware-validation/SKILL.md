---
name: hardware-validation
description: Test ecu-to-mqtt against a real ECU — detection, poll rate, outputs, dashboards — record fixtures safely, and never write to the controller.
---

# Hardware validation

## Before

- The gateway is read-only, and so is testing: no TunerStudio burns, no
  commands other than `Q`, `A`, `a 00 06`. Do not send `S`, page reads or
  anything else by hand either.
- Opening the port resets Arduino-based boards (DTR). Fine on a bench, so say
  so; on a running engine, ask first.
- Close TunerStudio: a serial port has one owner.
- `ecu-to-mqtt --list-ports --probe` shows the port and signature.
- Throwaway broker: `mosquitto -c conf` with `listener 18830 127.0.0.1`
  and `allow_anonymous true` in a temp directory.

## Checks

| What | How | Pass |
|------|-----|------|
| Detection | `--headless` with defaults | log shows `Detected ECU on … — speeduino 202501` |
| Rate and latency | `/metrics` counter deltas over ≥ 10 s (`ecu_samples_total`), `ecu_poll_latency_ms` | samples/s ≈ 1000 / `refresh_rate_ms`; 0 timeouts, 0 decode errors |
| Channels | `/api/v1/live` | plausible values; unconnected sensors absent, not zero |
| Status bits | Diagnostics mode | `sync` on with the engine turning; nothing spurious with it off |
| MQTT | `mosquitto_sub -p 18830 -t '#' -v -W 5` | every enabled format present, `<base>/status online` |
| Health | `--healthcheck`, `/health` | 0 with data, 1 with the ECU unplugged |
| Re-plug | unplug and replug USB | reconnects by itself, `ecu_reconnects_total` +1 |
| Shutdown | SIGINT | `<base>/status offline`, exit 0 |

Reference (DIY-EFI CORE4, 2025.01, CP210x): 50 Hz at 20 ms, ≈ 14 ms latency.
The 1 s gauges jitter; measure with counter deltas. macOS has no `timeout`; use
`mosquitto_sub -W` and `kill -INT`.

## Recording fixtures

1. Capture one raw `A` reply (a few lines of Rust or Python that write `A` and
   save the bytes). Engine data contains no personal information, but note the
   firmware and board.
2. Save as `tests/fixtures/<ecu>-<firmware>-<len>.bin` and add a decoder test
   asserting what the capture shows (length, signature-era fields, flags).

Stop any brokers and gateway processes you started.
