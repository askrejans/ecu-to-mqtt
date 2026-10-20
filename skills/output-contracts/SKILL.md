---
name: output-contracts
description: Add or change any ecu-to-mqtt output — MQTT topics/formats (canonical, legacy, json, homeassistant), HTTP/REST/SSE routes, Prometheus metrics, JSON fields — without breaking G86 consumers.
---

# Output contracts

## Consumers that break silently

| Contract | Consumer | Rule |
|----------|----------|------|
| `<base>/telemetry` canonical envelope: exactly `schema, source, partial, bootId, timestampMs, sequence, channels`; never retained | G86 Flutter app `lib/services/ecu/ecu_telemetry.dart` (2 s freshness, sequence/bootId checks, ignores unknown channels) | Frozen. Adding a channel is fine; renaming, rescaling or changing a unit needs a new `schema` and coordinated app work (`../../skills/automotive-telemetry`). The `sample.rs` test pins the key set. |
| Legacy topics `RPM MAP TPS BAT ADV AFT CAD MAT DWL ENG O2P PW1 VE1 WEC` (and the rest of `get_params_to_publish`) | G86 web dashboard (`g86-web-dashboard/src/pages/Dashboard.jsx`, `SmallDashboard.jsx`) | Keep names and number formats. Not retained; sent on change and re-sent every second (`LEGACY_REFRESH`) so late subscribers fill in. |
| `/metrics` names | Grafana | Add, never rename. Counters end in `_total`. |
| `/health` 200/503 semantics | Docker `HEALTHCHECK`, `--healthcheck` | Keep (`ok` / `no_data` / `ecu_down`, 2 s freshness). |

Check consumers before editing:
`grep -rn "<topic>" ../../g86-web-dashboard/src ../../g86-racing-app*/lib`.

## Adding data

1. New channel: see the ecu-protocols skill (`protocol::add()` range table).
   It then appears in the envelope, `/api/v1/live`, SSE, `json` state and
   `ecu_channel{name}`.
2. Dashboards: add a `CATALOG` row in `output/http.rs` (label, unit, group,
   gauge range), then the web-dashboard skill.
3. Home Assistant: add a sensor in `homeassistant_discovery` (template on
   `value_json.channels.<name>`, unit, device class). Keep `unique_id` stable.
4. Prometheus: add to the metrics renderer with HELP/TYPE.
5. Document in README (outputs) and TECHNICAL.md §4.

## Adding an MQTT format

Add a `MqttFormat` variant (list parsing is automatic), publish from
`Publisher::on_sample`/`publish_state`, re-send retained state in
`on_connect`, and use `try_publish` only while connected (drops counted).
Never block the publisher. `mqtt_max_rate_hz` applies to state formats, never
to `canonical`.

## HTTP

Routes under `/api/v1/*` sit behind the token middleware automatically;
`/health` and `/metrics` stay open. Test with `tower::ServiceExt::oneshot` (see
`output/http.rs` tests). Config goes out only via `AppConfig::redacted()`.
