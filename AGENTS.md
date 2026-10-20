# ecu-to-mqtt — agent guide

Read-only ECU telemetry gateway (Rust, tokio): Speeduino, MegaSquirt and CAN
ECUs to MQTT, HTTP and a web dashboard. Part of `g86racing`: read the
[project guidance](../../AGENTS.md) before changing anything another component
consumes. Preserve the current branch and any local edits.

Design, contracts, measurements and the security model:
[TECHNICAL.md](TECHNICAL.md). Packet layouts and profiles:
[ECU_PROTOCOLS.md](ECU_PROTOCOLS.md). User documentation: [README.md](README.md).

## Skills

Read the matching skill before starting:

| Task | Skill |
|------|-------|
| New or changed ECU profile, decoder, channel, status bit, source or poll behaviour | [skills/ecu-protocols](skills/ecu-protocols/SKILL.md) |
| New or changed output: MQTT format/topic, HTTP route, metric, JSON fields | [skills/output-contracts](skills/output-contracts/SKILL.md) |
| Web dashboard (`web/`) | [skills/web-dashboard](skills/web-dashboard/SKILL.md) |
| Testing with a real ECU, recording fixtures | [skills/hardware-validation](skills/hardware-validation/SKILL.md) |
| Version bump, packages, Docker, dependencies, audit | [skills/release](skills/release/SKILL.md) |
| Cross-component telemetry contracts (G86 app, dashboard, API) | [../../skills/automotive-telemetry](../../skills/automotive-telemetry/SKILL.md) |
| Lap/sector timing consumers of this data | [../../skills/race-timing-analysis](../../skills/race-timing-analysis/SKILL.md) |

## Map

```
src/protocol/      profile registry + range table (mod.rs), MegaSquirt 112-byte,
                   speeduino.rs (packet, canonical, status bits, legacy topics,
                   encoder), can_profiles.rs                ← untrusted input
src/source/        poller (Q identify, A / a 00 06 loop), link (serial/TCP,
                   port ranking), can_socket, can_gateway, can_input, simulator
src/hub.rs         single consumer of SourceEvents → sequence, snapshot, bus, archive
src/sample.rs      Sample, Stamped, canonical envelope (G86 contract)
src/shared.rs      snapshot, counters, health
src/output/        mqtt, http (+ web.rs embedded assets)
src/tui/           ratatui terminal dashboard
web/               browser dashboard, embedded with include_str!
tests/fixtures/    real ECU captures
scripts/           build_packages.sh, can_gateway.py, smoke_mqtt_bridge.py
```

## Commands

```bash
cargo test                                    # must pass; includes the real-ECU capture
cargo clippy --all-targets -- -D warnings     # must be clean
cargo fmt
cargo audit                                   # exceptions only in .cargo/audit.toml, with a reason
cargo run -- --simulate                       # no hardware needed
cargo run -- --list-ports --probe             # sends only `Q`
cargo build && python3 scripts/smoke_mqtt_bridge.py   # all profiles → real broker
```

## Invariants

1. **The ECU is read-only.** Only `Q`, `A` (Speeduino) and `a 00 06`
   (MegaSquirt) are ever written, and CAN is never transmitted. No tuning,
   burn, flash or page reads, not even behind a flag. The fake-ECU tests fail
   on any other byte; keep them that way.
2. **Acquisition never waits on an output.** Outputs subscribe to the lossy
   bus; drops are counted. Only the archive has its own bounded queue.
3. **One decode path.** Sources produce `Sample`s through the profile decoders;
   the hub alone sequences them and builds the envelope. The simulator encodes
   real packets and goes through the same decoder.
4. **Untrusted input is bounded and never panics.** ECU bytes, bridge and
   gateway peers, HTTP clients. No `unwrap` on input, no unchecked slicing.
5. **Channels go through `protocol::add()`.** It range-checks and rounds;
   implausible readings are omitted, never zero-filled. Units are in the name.
6. **Contracts are frozen unless versioned.** The canonical envelope (schema 1,
   seven keys), channel names and units, and the legacy topic names/formats
   read by the G86 dashboard. See the output-contracts skill.
7. **The web dashboard has no remote resources and no `innerHTML`.** A test
   enforces this. Configuration is read-only.
8. **Config is flat** (`mqtt_host`, not `[mqtt] host`) so every key maps to
   `ECU_TO_MQTT_<KEY>`. New keys need a serde default, validation,
   `example.settings.toml`, the README table, and a test.

## Conventions

- Edition 2024, `anyhow` for application errors, `thiserror` for typed ones
  (`errors.rs`), `tracing` for logs. Rate-limit anything that can fire at poll
  frequency (`source::RateLimit`).
- std `RwLock` for the snapshot, never held across `.await`.
- Tests sit next to the code (`#[cfg(test)] mod tests`). Prefer real fixtures
  and local sockets over mocks.
- Comments explain *why*: firmware quirks, specifications, measured behaviour.
- Licence: GPL-3.0-only.
