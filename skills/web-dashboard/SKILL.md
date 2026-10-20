---
name: web-dashboard
description: Change the ecu-to-mqtt browser dashboard in web/ (modes, gauges, lamps, charts, themes, styling) while keeping its CSP, security and no-build constraints.
---

# Web dashboard

`web/index.html`, `web/app.css`, `web/app.js` and `web/icon.svg` are embedded
with `include_str!` (`src/output/web.rs`). Edit, then `cargo build`. There is
no bundler, framework or npm; keep it that way.

## Hard rules (enforced by tests in `output/web.rs`)

- No inline `<script>`, no `innerHTML`, no `http(s)://` in the assets. Build
  DOM with `el()`/`kv()` and set text with `textContent`.
- Only `'self'` in the CSP. No external fonts, scripts or images.
- Data only via the public API (`api/v1/*`, relative paths for reverse
  proxies); `Authorization` via `headers()`; SSE is read with `fetch`. The UI
  is read-only: no endpoints that change config, and nothing that could reach
  the ECU.

## Structure of app.js

State `S` → `schedule()` (one rAF) → `render()` draws the bar, the readout and
**only the visible mode** (`MODES`: dash, engine, charts, diag, config). Data
arrives via `onSample` (SSE; also appends chart history), a status poll and
`loadOnce` (channel catalogue, config). Gauges come from the catalogue ranges
and `zoneFor()`; Dash lamps from the `LAMPS` table of `[group, flag, label,
severity]`; Engine fields from `FIELDS`. Add a mode: nav key,
`<section class="mode" id="mode-x" hidden>`, `MODES` entry, render function.

## Themes

Light and dark are both first-class. Every colour is a CSS variable in
`app.css` (`:root` dark, `:root[data-theme="light"]` light). Canvases read
colours through `themeColours()`, never hard-coded. `applyTheme()` stores the
choice in `localStorage` ("theme"); the default follows
`prefers-color-scheme`. There is no dim/night mode. Check every change in
both themes.

## Style

Professional instrument panel: function over form, no decorative text or
effects. Uppercase micro-labels, numeric font with `tabular-nums`, status from
lamps (`ok` / `warn` / `bad`) and segment meters. Reserve width for values that
change length.

## Verify

`ECU_TO_MQTT_MQTT_ENABLED=false ECU_TO_MQTT_HTTP_ENABLED=true cargo run --release -- --simulate --headless`,
open `http://127.0.0.1:9091/`, check every mode in light and dark, at desktop
and phone widths, with and without `ECU_TO_MQTT_HTTP_TOKEN`. Assets are
compiled in: restart after rebuilding and hard-reload.
