/* ecu-to-mqtt web dashboard.
 *
 * Data: GET api/v1/{status,live,channels,config} and the api/v1/stream SSE
 * feed (read with fetch so the bearer token travels in a header). Dynamic
 * text is set with textContent only. Only the visible mode is redrawn.
 */
"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const store = {
    get(k, d) { try { return localStorage.getItem(k) ?? d; } catch { return d; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { /* blocked */ } },
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // Canvas colours follow the active theme's CSS variables.
  const C = { font: '"Chakra Petch", ui-sans-serif, system-ui, sans-serif', num: '"Doto", "JetBrains Mono", ui-monospace, monospace' };
  function themeColours() {
    const css = getComputedStyle(document.documentElement);
    for (const k of ["display", "cyan", "amber", "green", "red", "text", "muted", "faint"]) C[k] = css.getPropertyValue(`--${k}`).trim();
    C.grid = css.getPropertyValue("--line").trim();
  }
  const MODES = ["dash", "engine", "charts", "diag", "config"];
  const STALE_MS = 2000;
  const TEMP = new Set(["coolantC", "intakeC", "oilC", "fuelC", "transmissionC", "differentialC"]);
  const DASH = ["rpm", "throttle", "manifoldKpa", "boostKpa", "afr", "lambda", "coolantC", "intakeC", "oilPressureKpa", "oilC",
    "batteryV", "ignitionDeg", "ecuSpeedKmh", "gear", "fuelPressureKpa", "injectorPwMs", "dwellMs", "veCurrent"];
  const CHART_DEFAULT = ["rpm", "throttle", "manifoldKpa", "afr", "coolantC", "ignitionDeg"];

  const S = {
    mode: MODES.includes(location.hash.slice(1)) ? location.hash.slice(1) : store.get("mode", "dash"),
    fahrenheit: store.get("temp") === "F",
    theme: store.get("theme") || (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"),
    token: store.get("token", ""),
    catalog: {}, config: null, status: null,
    channels: {}, detail: null, flags: [], protocol: null, firmware: null,
    lastAt: 0, streamUp: false,
    history: {}, window: Number(store.get("chartWindow", 60)) || 60, paused: false,
    charted: (store.get("charted") || CHART_DEFAULT.join(",")).split(",").filter(Boolean),
    prevCounters: null, prevAt: 0, rates: {},
  };

  // ── formatting ────────────────────────────────────────────────────────
  const isNum = (v) => typeof v === "number" && Number.isFinite(v);
  const fmt = (v, d = 1) => (isNum(v) ? v.toFixed(d) : "—");
  const tempOut = (name, v) => (TEMP.has(name) && S.fahrenheit && isNum(v) ? v * 9 / 5 + 32 : v);
  const unitOf = (name) => (TEMP.has(name) ? (S.fahrenheit ? "°F" : "°C") : S.catalog[name]?.unit ?? "");
  const labelOf = (name) => S.catalog[name]?.label ?? name;
  const DECIMALS = { rpm: 0, throttle: 0, manifoldKpa: 0, boostKpa: 0, baroKpa: 0, afr: 1, afrTarget: 1, lambda: 3, lambda2: 3,
    coolantC: 0, intakeC: 0, oilC: 0, fuelC: 0, batteryV: 1, ignitionDeg: 0, ecuSpeedKmh: 0, gear: 0, veCurrent: 0,
    injectorPwMs: 2, dwellMs: 1, oilPressureKpa: 0, fuelPressureKpa: 0, syncLossCount: 0 };
  const show = (name, v) => fmt(tempOut(name, v), DECIMALS[name] ?? 2);
  const age = (name) => { const c = S.channels[name]; return c ? performance.now() - c.at : Infinity; };
  const val = (name) => (age(name) <= STALE_MS ? S.channels[name].value : null);

  // ── DOM helpers ───────────────────────────────────────────────────────
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function setText(id, v) { const e = $(id); if (e && e.textContent !== v) e.textContent = v; }
  function kv(id, rows) {
    const frag = document.createDocumentFragment();
    for (const [k, v, cls, small] of rows) {
      frag.append(el("dt", null, k));
      const dd = el("dd", cls || null, v);
      if (small) dd.append(el("small", null, small));
      frag.append(dd);
    }
    $(id).replaceChildren(frag);
  }
  function meterInto(box, frac, n, zone) {
    if (box.children.length !== n) box.replaceChildren(...Array.from({ length: n }, () => el("i")));
    const lit = Math.round(Math.max(0, Math.min(1, frac)) * n);
    for (let i = 0; i < n; i++) box.children[i].className = i < lit ? `on ${zone(i / n)}` : "";
  }

  // ── API ───────────────────────────────────────────────────────────────
  const headers = () => (S.token ? { Authorization: `Bearer ${S.token}` } : {});
  async function api(path) {
    const res = await fetch(path, { headers: headers(), cache: "no-store" });
    if (res.status === 401) { askToken(); throw new Error("unauthorized"); }
    return res;
  }
  function askToken() { const d = $("auth"); if (!d.open) d.showModal(); }
  $("auth").addEventListener("close", () => {
    const t = $("auth-token").value.trim();
    if (!t) return;
    S.token = t; store.set("token", t); $("auth-token").value = "";
    restartStream(); loadOnce(); pollStatus();
  });

  let streamCtl = null;
  async function stream() {
    let delay = 1000;
    for (;;) {
      const ctl = new AbortController();
      streamCtl = ctl;
      try {
        const res = await fetch("api/v1/stream", { headers: { ...headers(), Accept: "text/event-stream" }, signal: ctl.signal, cache: "no-store" });
        if (res.status === 401) { setStream(false); askToken(); return; }
        if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
        setStream(true); delay = 1000;
        const reader = res.body.getReader(), dec = new TextDecoder();
        let buf = "";
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          buf += dec.decode(value, { stream: true }).replace(/\r\n/g, "\n");
          let i;
          while ((i = buf.indexOf("\n\n")) >= 0) { dispatch(buf.slice(0, i)); buf = buf.slice(i + 2); }
        }
      } catch {
        if (ctl.signal.aborted) return;
      }
      setStream(false);
      await sleep(delay); delay = Math.min(delay * 2, 15000);
    }
  }
  function restartStream() { if (streamCtl) streamCtl.abort(); stream(); }
  function setStream(up) { S.streamUp = up; schedule(); }
  function dispatch(block) {
    let ev = "message", data = "";
    for (const line of block.split("\n")) {
      if (line.startsWith("event:")) ev = line.slice(6).trim();
      else if (line.startsWith("data:")) data += line.slice(5).trimStart();
    }
    if (ev !== "sample" || !data) return;
    try { onSample(JSON.parse(data)); } catch { /* malformed */ }
  }

  function onSample(s) {
    const now = performance.now();
    if (!s.partial) S.channels = {};
    for (const [k, v] of Object.entries(s.channels || {})) {
      S.channels[k] = { value: v, at: now };
      if (!S.paused) {
        const h = (S.history[k] ||= []);
        h.push([now, v]);
      }
    }
    if (s.speeduino) { S.detail = s.speeduino; S.flags = s.flags || []; }
    S.lastAt = now;
    schedule();
  }

  function trimHistory() {
    const cutoff = performance.now() - 125000;
    for (const h of Object.values(S.history)) {
      let i = 0;
      while (i < h.length && h[i][0] < cutoff) i++;
      if (i) h.splice(0, i);
    }
  }

  async function loadOnce() {
    try {
      const [cat, cfg, live] = await Promise.all([api("api/v1/channels"), api("api/v1/config"), api("api/v1/live")]);
      if (cat.ok) for (const c of await cat.json()) S.catalog[c.name] = c;
      if (cfg.ok) S.config = await cfg.json();
      if (live.ok) {
        const l = await live.json(), now = performance.now();
        for (const [k, c] of Object.entries(l.channels || {})) S.channels[k] = { value: c.value, at: now - c.ageMs };
        if (l.speeduino) { S.detail = l.speeduino; S.flags = l.flags || []; }
      }
    } catch { /* retried by pollStatus */ }
    configRendered = false;
    schedule();
  }

  async function pollStatus() {
    try {
      const r = await api("api/v1/status");
      if (r.ok) {
        const st = await r.json(), now = performance.now();
        if (S.prevCounters) {
          const dt = (now - S.prevAt) / 1000;
          for (const [k, v] of Object.entries(st.counters)) S.rates[k] = (v - (S.prevCounters[k] ?? v)) / dt;
        }
        S.prevCounters = st.counters; S.prevAt = now; S.status = st;
        S.protocol = st.health.protocol; S.firmware = st.health.firmware;
        if (!S.config) loadOnce();
      }
    } catch { /* lamps show it */ }
    schedule();
  }

  // ── rendering ─────────────────────────────────────────────────────────
  let queued = false;
  function schedule() { if (!queued) { queued = true; requestAnimationFrame(render); } }
  function render() {
    queued = false;
    renderBar(); renderReadout();
    ({ dash: renderDash, engine: renderEngine, charts: renderCharts, diag: renderDiag, config: renderConfig })[S.mode]();
  }

  const flag = (group, name) => S.flags.some((f) => f.group === group && f.name === name && f.on);
  function lamp(id, state) { $(id).className = `lamp ${state}`; }

  function renderBar() {
    const st = S.status, h = st?.health;
    setText("version", st?.info?.version ? `v${st.info.version}` : "");
    const fresh = performance.now() - S.lastAt <= STALE_MS;
    lamp("lamp-ecu", !h ? "" : h.link === "connected" ? (fresh ? "ok" : "warn") : h.link === "connecting" ? "warn" : "bad");
    const rpm = val("rpm");
    lamp("lamp-engine", !fresh ? "" : (S.detail ? flag("engine", "running") : rpm > 0) ? "ok" : "");
    lamp("lamp-mqtt", !h || h.mqtt === "disabled" ? "" : h.mqtt === "connected" ? "ok" : h.mqtt === "connecting" ? "warn" : "bad");
    lamp("lamp-link", S.streamUp ? "ok" : "bad");
    setText("fw", S.firmware ? `${S.firmware}` : S.protocol || "—");
    setText("foot-source", h ? `${h.link_detail || st.info.connection} · ${h.protocol}` : "connecting…");
    if (h) setText("foot-uptime", `up ${Math.floor(h.uptime_s / 3600)}h ${String(Math.floor(h.uptime_s / 60) % 60).padStart(2, "0")}m`);
  }

  function renderReadout() {
    document.querySelector(".readout").classList.toggle("stale", performance.now() - S.lastAt > STALE_MS);
    setText("r-rpm", show("rpm", val("rpm")));
    const d = S.detail;
    setText("r-rpm-sub", d ? `${d.loops_per_second} loops/s · sync losses ${d.sync_loss_counter}` : " ");
    setText("r-tps", show("throttle", val("throttle")));
    setText("r-map", show("manifoldKpa", val("manifoldKpa")));
    const boost = val("boostKpa");
    setText("r-map-sub", isNum(boost) ? `boost ${boost >= 0 ? "+" : ""}${boost.toFixed(0)} kPa` : " ");
    const lam = val("lambda");
    setText("r-afr", isNum(val("afr")) ? show("afr", val("afr")) : isNum(lam) ? `λ ${lam.toFixed(3)}` : "—");
    setText("r-afr-sub", isNum(val("afrTarget")) ? `target ${val("afrTarget").toFixed(1)}` : " ");
    setText("r-clt", show("coolantC", val("coolantC")));
    $("r-clt").parentElement.querySelector("small").textContent = unitOf("coolantC");
    setText("r-clt-sub", isNum(val("intakeC")) ? `intake ${show("intakeC", val("intakeC"))} ${unitOf("intakeC")}` : " ");
    setText("r-bat", show("batteryV", val("batteryV")));
    const h = S.status?.health;
    setText("r-rate", h ? fmt(h.sample_rate_hz, 0) : "—");
    setText("r-rate-sub", h && isNum(h.poll_latency_ms) ? `${h.poll_latency_ms.toFixed(1)} ms per poll` : " ");
  }

  // Dash ------------------------------------------------------------------
  const LAMPS = [
    ["engine", "running", "Running", ""], ["engine", "cranking", "Cranking", "a"], ["status2", "sync", "Sync", ""],
    ["engine", "warmupEnrich", "Warm-up", "a"], ["engine", "afterStartEnrich", "ASE", "a"], ["engine", "tpsAccel", "Accel enrich", "a"],
    ["status1", "dfco", "DFCO", ""], ["status2", "idleControl", "Idle ctrl", ""], ["status2", "softRevLimit", "Soft limit", "a"],
    ["status2", "hardRevLimit", "Hard limit", "r"], ["status2", "softLaunch", "Launch", "a"], ["status2", "boostCutSpark", "Boost cut", "r"],
    ["status2", "sparkError", "Spark error", "r"], ["status5", "knockActive", "Knock", "r"], ["status4", "fan", "Fan", ""],
  ];
  function zoneFor(name, v) {
    if (!isNum(v)) return () => "";
    if (name === "coolantC") return () => (v > 105 ? "r" : v > 98 ? "a" : "");
    if (name === "batteryV") return () => (v < 11.5 || v > 15.2 ? "r" : v < 12.3 ? "a" : "");
    if (name === "rpm") return (f) => (f > 0.85 ? "r" : f > 0.72 ? "a" : "");
    if (name === "oilPressureKpa") return () => (v < 80 && val("rpm") > 1000 ? "r" : "");
    return () => "";
  }
  function renderDash() {
    const lampsBox = $("dash-lamps");
    if (S.detail) {
      const protect = S.flags.filter((f) => f.group === "engineProtect" && f.on).map((f) => f.name);
      const items = LAMPS.map(([g, n, label, sev]) => {
        const on = flag(g, n);
        const e = el("span", `ind${on ? ` on ${sev}` : ""}`); e.append(el("i"), document.createTextNode(label)); return e;
      });
      const p = el("span", `ind${protect.length ? " on r" : ""}`);
      p.append(el("i"), document.createTextNode(protect.length ? `Protect: ${protect.join(", ")}` : "Protect"));
      items.push(p);
      if (flag("engine", "running") && !flag("status2", "sync")) {
        const ns = el("span", "ind on r"); ns.append(el("i"), document.createTextNode("No sync")); items.unshift(ns);
      }
      lampsBox.replaceChildren(...items);
    } else {
      lampsBox.replaceChildren(el("span", "note", `Status bits are provided by Speeduino; ${S.protocol || "this profile"} publishes channels only.`));
    }
    const present = DASH.filter((n) => S.channels[n]);
    const box = $("dash-gauges");
    if (box.dataset.keys !== present.join(",")) {
      box.dataset.keys = present.join(",");
      box.replaceChildren(...present.map((n) => {
        const a = el("article", `panel gauge${n === "rpm" ? " hero" : ""}`);
        a.dataset.name = n;
        const v = el("span", "val"); v.append(el("b", null, "—"), el("small", null, unitOf(n)));
        const scale = el("div", "scale"); scale.append(el("span"), el("span"));
        a.append(el("span", "lbl", labelOf(n)), v, el("div", "meter"), scale, el("span", "sub", " "));
        return a;
      }));
    }
    for (const card of box.children) {
      const n = card.dataset.name, c = S.catalog[n], raw = val(n), v = tempOut(n, raw);
      card.classList.toggle("stale", raw == null);
      card.querySelector("b").textContent = show(n, raw);
      card.querySelector("small").textContent = unitOf(n);
      const lo = tempOut(n, c?.min ?? 0), hi = tempOut(n, c?.max ?? 100);
      meterInto(card.querySelector(".meter"), isNum(v) ? (v - lo) / (hi - lo) : 0, n === "rpm" ? 40 : 24, zoneFor(n, raw));
      const [a, b] = card.querySelectorAll(".scale span");
      a.textContent = fmt(lo, 0); b.textContent = fmt(hi, 0);
      const sub = card.querySelector(".sub");
      sub.textContent = n === "afr" && isNum(val("afrTarget")) ? `target ${val("afrTarget").toFixed(1)}`
        : n === "manifoldKpa" && isNum(val("baroKpa")) ? `baro ${val("baroKpa").toFixed(0)} kPa`
          : raw == null && S.channels[n] ? `stale ${(age(n) / 1000).toFixed(1)} s` : " ";
    }
  }

  // Engine ----------------------------------------------------------------
  const pct = (v) => [v, "%"], raw = (v) => [v, ""], kpa = (v) => [v, "kPa"], deg = (v) => [v, "°"];
  const FIELDS = [
    ["Engine", [["rpm", "Engine speed", (v) => [v, "rpm"]], ["rpm_dot", "RPM rate", (v) => [v, "rpm/s"]], ["map", "MAP", kpa], ["baro", "Barometer", kpa],
      ["emap", "EMAP", kpa], ["tps", "Throttle", pct], ["tps_dot", "Throttle rate", (v) => [v / 10, "%/s"]], ["tps_adc", "TPS ADC", raw],
      ["map_dot", "MAP rate", (v) => [v / 10, "kPa/s"]], ["fuel_load", "Fuel load", raw], ["ign_load", "Ignition load", raw], ["gear", "Gear", raw], ["vss", "Vehicle speed", (v) => [v, "km/h"]]]],
    ["Fuel", [["pw1", "Pulse width 1", (v) => [v / 10, "ms"]], ["pw2", "Pulse width 2", (v) => [v / 10, "ms"]], ["pw3", "Pulse width 3", (v) => [v / 10, "ms"]],
      ["pw4", "Pulse width 4", (v) => [v / 10, "ms"]], ["ve1", "VE 1", pct], ["ve2", "VE 2", pct], ["ve_current", "VE current", pct],
      ["afr_target", "AFR target", (v) => [v / 10, ""]], ["o2_primary", "O2 primary", (v) => [v / 10, "AFR"]], ["o2_secondary", "O2 secondary", (v) => [v / 10, "AFR"]],
      ["corrections", "Total correction", pct], ["ego_correction", "EGO correction", pct], ["iat_correction", "IAT correction", pct], ["wue_correction", "Warm-up enrich", pct],
      ["bat_correction", "Battery correction", pct], ["baro_correction", "Baro correction", pct], ["ase_value", "After-start enrich", pct], ["tae_amount_raw", "Accel enrich", (v) => [v * 2, "%"]],
      ["fuel_pressure", "Fuel pressure", kpa], ["ethanol_pct", "Ethanol", pct], ["flex_correction", "Flex fuel corr.", pct], ["fuel_temp_correction", "Fuel temp corr.", pct]]],
    ["Ignition", [["advance", "Advance", deg], ["advance1", "Advance table 1", deg], ["advance2", "Advance table 2", deg], ["dwell", "Dwell", (v) => [v / 10, "ms"]],
      ["actual_dwell", "Measured dwell", (v) => [v / 10, "ms"]], ["flex_ign_correction", "Flex ign. corr.", deg], ["knock_count", "Knock count", raw], ["knock_retard", "Knock retard", deg]]],
    ["Temperatures & pressures", [["coolant_raw", "Coolant", (v) => tempPair("coolantC", v - 40)], ["iat_raw", "Intake air", (v) => tempPair("intakeC", v - 40)],
      ["fuel_temp_raw", "Fuel", (v) => tempPair("fuelC", v - 40)], ["oil_pressure", "Oil pressure", kpa], ["battery_10", "Battery", (v) => [v / 10, "V"]]]],
    ["Boost, idle, VVT", [["boost_target_raw", "Boost target", (v) => [v * 2, "kPa"]], ["boost_duty_raw", "Boost duty", (v) => [v * 100, "%"]], ["flex_boost_correction", "Flex boost corr.", kpa],
      ["idle_load", "Idle load", raw], ["cl_idle_target", "Idle target", (v) => [v * 10, "rpm"]], ["vvt1_angle", "VVT1 angle", deg], ["vvt1_target_angle", "VVT1 target", deg],
      ["vvt1_duty", "VVT1 duty", pct], ["vvt2_angle", "VVT2 angle", deg], ["vvt2_target_angle", "VVT2 target", deg], ["vvt2_duty", "VVT2 duty", pct], ["fan_duty", "Fan duty", pct], ["wmi_pw", "WMI pulse width", raw]]],
    ["Controller", [["secl", "Seconds counter", (v) => [v, "s"]], ["loops_per_second", "Main loops", (v) => [v, "/s"]], ["free_ram", "Free RAM", (v) => [v, "B"]],
      ["sync_loss_counter", "Sync losses", raw], ["next_error", "Next error code", raw], ["test_outputs", "Test outputs", raw], ["ts_sd_status", "SD card status", raw]]],
  ];
  function tempPair(name, c) { return [tempOut(name, c), unitOf(name)]; }
  function renderEngine() {
    const now = performance.now();
    const names = Object.keys(S.channels).sort();
    $("ch-table").tBodies[0].replaceChildren(...names.map((n) => {
      const c = S.channels[n], a = now - c.at, tr = el("tr", a > STALE_MS ? "stale" : null);
      tr.append(el("td", null, labelOf(n)), el("td", "n v", show(n, c.value)), el("td", null, unitOf(n)), el("td", "n age", a < 10000 ? `${(a / 1000).toFixed(1)} s` : "—"));
      return tr;
    }));
    if (!S.detail) {
      setText("detail-note", "available for the Speeduino profile");
      $("detail-groups").replaceChildren();
      return;
    }
    setText("detail-note", "full realtime packet");
    const d = S.detail;
    $("detail-groups").replaceChildren(...FIELDS.map(([title, fields]) => {
      const sec = el("section"), dl = el("dl", "kv");
      sec.append(el("h3", null, title), dl);
      for (const [key, label, f] of fields) {
        if (d[key] == null) continue;
        const [v, unit] = f(d[key]);
        const dd = el("dd", null, typeof v === "number" ? String(Math.round(v * 100) / 100) : String(v));
        if (unit) dd.append(el("small", null, unit));
        dl.append(el("dt", null, label), dd);
      }
      return sec;
    }));
  }

  // Charts ----------------------------------------------------------------
  function renderPicker() {
    const names = Object.keys(S.catalog).filter((n) => S.channels[n]);
    const box = $("chart-picker");
    if (box.dataset.keys === names.join(",")) return;
    box.dataset.keys = names.join(",");
    box.replaceChildren(...names.map((n) => {
      const l = el("label"), cb = el("input");
      cb.type = "checkbox"; cb.checked = S.charted.includes(n);
      cb.addEventListener("change", () => {
        S.charted = cb.checked ? [...S.charted, n] : S.charted.filter((x) => x !== n);
        store.set("charted", S.charted.join(",")); schedule();
      });
      l.append(cb, document.createTextNode(labelOf(n)));
      return l;
    }));
  }
  function renderCharts() {
    renderPicker();
    document.querySelectorAll("[data-window]").forEach((k) => k.classList.toggle("active", Number(k.dataset.window) === S.window));
    const shown = S.charted.filter((n) => S.channels[n]);
    const box = $("charts");
    if (box.dataset.keys !== shown.join(",")) {
      box.dataset.keys = shown.join(",");
      box.replaceChildren(...shown.map((n) => {
        const a = el("article", "panel chart"); a.dataset.name = n;
        const lg = el("div", "legend-box"); lg.append(el("span", "lbl", labelOf(n)), el("b", null, "—"), el("span", "sub", unitOf(n)));
        a.append(lg, el("canvas"));
        return a;
      }));
    }
    setText("chart-note", `${S.window} s window${S.paused ? " · paused" : ""}`);
    const now = performance.now(), span = S.window * 1000;
    for (const card of box.children) {
      const n = card.dataset.name, h = S.history[n] || [];
      card.querySelector("b").textContent = show(n, val(n));
      card.querySelector(".sub").textContent = unitOf(n);
      drawChart(card.querySelector("canvas"), h.filter((p) => p[0] >= now - span), now, span, n);
    }
  }
  function drawChart(cv, pts, now, span, name) {
    const dpr = window.devicePixelRatio || 1, w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    if (cv.width !== Math.round(w * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); }
    const ctx = cv.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, w, h);
    if (pts.length < 2) { ctx.fillStyle = C.faint; ctx.font = `12px ${C.font}`; ctx.fillText("collecting…", 10, h / 2); return; }
    let lo = Infinity, hi = -Infinity;
    for (const [, v] of pts) { const t = tempOut(name, v); if (t < lo) lo = t; if (t > hi) hi = t; }
    if (hi - lo < 1e-6) { hi += 1; lo -= 1; }
    const pad = (hi - lo) * 0.1; lo -= pad; hi += pad;
    const x = (t) => w - ((now - t) / span) * w, y = (v) => h - 4 - ((tempOut(name, v) - lo) / (hi - lo)) * (h - 8);
    ctx.strokeStyle = C.grid; ctx.lineWidth = 1; ctx.beginPath();
    for (let i = 1; i < 4; i++) { ctx.moveTo(0, (h * i) / 4); ctx.lineTo(w, (h * i) / 4); }
    ctx.stroke();
    ctx.fillStyle = C.muted; ctx.font = `10px ${C.num}`;
    ctx.fillText(fmt(hi, 1), 4, 12); ctx.fillText(fmt(lo, 1), 4, h - 4);
    ctx.strokeStyle = C.cyan; ctx.lineWidth = 1.6; ctx.beginPath();
    pts.forEach(([t, v], i) => (i ? ctx.lineTo(x(t), y(v)) : ctx.moveTo(x(t), y(v))));
    ctx.stroke();
  }

  // Diagnostics -----------------------------------------------------------
  const SEVERITY = { hardRevLimit: "bad", sparkError: "bad", boostCutSpark: "bad", boostCutFuel: "bad", knockActive: "bad", vvt1Error: "bad", vvt2Error: "bad",
    wmiEmpty: "warn", softRevLimit: "warn", cranking: "warn", burnPending: "warn" };
  const GROUP_LABEL = { engine: "Engine", status1: "Status 1", status2: "Status 2", status3: "Status 3", status4: "Status 4", status5: "Status 5",
    engineProtect: "Engine protection", airCon: "Air conditioning" };
  function renderDiag() {
    if (S.flags.length) {
      const groups = {};
      for (const f of S.flags) (groups[f.group] ||= []).push(f);
      $("diag-flags").replaceChildren(...Object.entries(groups).map(([g, list]) => {
        const sec = el("section"), ul = el("ul");
        sec.append(el("h3", null, GROUP_LABEL[g] || g), ul);
        for (const f of list) {
          const sev = g === "engineProtect" ? "bad" : SEVERITY[f.name] || "";
          const li = el("li", f.on ? `on ${sev}` : null); li.append(el("i"), document.createTextNode(f.name)); ul.append(li);
        }
        return sec;
      }));
    } else {
      $("diag-flags").replaceChildren(el("p", "note", "Status bits are available for the Speeduino profile."));
    }
    const h = S.status?.health;
    kv("diag-link", [
      ["Link", h?.link || "—", h?.link === "connected" ? "ok" : "warn", h?.link_detail || ""],
      ["Profile", h?.protocol || "—"],
      ["Firmware", h?.firmware || "—", "hl"],
      ["Sample rate", h ? `${fmt(h.sample_rate_hz, 0)} Hz` : "—"],
      ["Poll latency", h && isNum(h.poll_latency_ms) ? `${h.poll_latency_ms.toFixed(1)} ms` : "—"],
      ["Sample age", h && isNum(h.sample_age_s) ? `${(h.sample_age_s * 1000).toFixed(0)} ms` : "—", h && h.sample_age_s > 2 ? "warn" : null],
      ["MQTT", h?.mqtt || "—", h?.mqtt === "connected" ? "ok" : h?.mqtt === "disabled" ? null : "warn"],
    ]);
    const c = S.status?.counters || {}, r = (k) => (isNum(S.rates[k]) ? `${S.rates[k].toFixed(1)}/s` : "");
    kv("diag-counters", [
      ["Samples", String(c.samples ?? "—"), null, r("samples")],
      ["Decode errors", String(c.decode_errors ?? "—"), c.decode_errors ? "warn" : null, r("decode_errors")],
      ["Timeouts", String(c.timeouts ?? "—"), c.timeouts ? "warn" : null, r("timeouts")],
      ["Reconnects", String(c.reconnects ?? "—"), c.reconnects ? "warn" : null],
      ["MQTT published", String(c.mqtt_published ?? "—"), null, r("mqtt_published")],
      ["MQTT dropped", String(c.mqtt_dropped ?? "—"), c.mqtt_dropped ? "warn" : null, r("mqtt_dropped")],
      ["Archive errors", String(c.archive_errors ?? "—"), c.archive_errors ? "bad" : null],
    ]);
    const d = S.detail;
    kv("diag-ecu", d ? [
      ["Main loops", `${d.loops_per_second}/s`], ["Free RAM", `${d.free_ram} B`], ["Sync losses", String(d.sync_loss_counter), d.sync_loss_counter ? "warn" : null],
      ["Next error", String(d.next_error), d.next_error ? "warn" : null], ["Seconds counter", `${d.secl} s`],
    ] : [["Controller", "details available for Speeduino"]]);
  }

  // Configuration ---------------------------------------------------------
  const GROUPS = [
    ["ECU", /^(ecu_protocol|can_base_id|expected_data_length|refresh_rate_ms|read_timeout_ms)$/],
    ["Connection", /^(connection_type|port_name|baud_rate|tcp_|can_interface)/],
    ["Reconnect", /^(max_retry_count|initial_retry_delay_ms|max_retry_delay_ms)$/],
    ["MQTT", /^(mqtt_|message_buffer_size)/],
    ["HTTP", /^http_/],
    ["Archive & logging", /^(telemetry_log|log_)/],
  ];
  let configRendered = false;
  function renderConfig() {
    const st = S.status;
    if (st) {
      kv("cfg-wiring", [["Profile", st.info.profile], ["Connection", st.info.connection], ["MQTT", st.info.mqtt || "disabled"],
        ...st.info.outputs.map((o, i) => [i ? "" : "Outputs", o])]);
    }
    if (!$("cfg-endpoints").children.length) {
      const eps = [["health", "open"], ["metrics", "open, Prometheus"], ["api/v1/status", "token"], ["api/v1/live", "token"],
        ["api/v1/channels", "token"], ["api/v1/config", "token"], ["api/v1/stream", "token, SSE"]];
      $("cfg-endpoints").replaceChildren(...eps.map(([p, note]) => {
        const li = el("li"), a = el("a", null, `/${p}`); a.href = p; a.target = "_blank"; a.rel = "noopener";
        li.append(a, el("small", null, note)); return li;
      }));
    }
    if (!S.config || configRendered) return;
    configRendered = true;
    const used = new Set();
    const sections = GROUPS.map(([title, re]) => {
      const keys = Object.keys(S.config).filter((k) => re.test(k) && !used.has(k)).sort();
      keys.forEach((k) => used.add(k));
      return [title, keys];
    });
    const rest = Object.keys(S.config).filter((k) => !used.has(k) && k !== "config_path");
    if (rest.length) sections.push(["Other", rest]);
    $("cfg-groups").replaceChildren(...sections.filter(([, k]) => k.length).map(([title, keys]) => {
      const sec = el("section"), dl = el("dl", "kv");
      sec.append(el("h3", null, title), dl);
      for (const k of keys) {
        const v = S.config[k];
        dl.append(el("dt", null, k), el("dd", v == null ? "unset" : null, v == null ? "unset" : Array.isArray(v) ? v.join(", ") : String(v)));
      }
      return sec;
    }));
  }

  // ── controls ──────────────────────────────────────────────────────────
  function setMode(m) {
    S.mode = m; store.set("mode", m);
    if (location.hash !== `#${m}`) history.replaceState(null, "", `#${m}`);
    for (const id of MODES) $(`mode-${id}`).hidden = id !== m;
    document.querySelectorAll(".key[data-mode]").forEach((k) => k.classList.toggle("active", k.dataset.mode === m));
    schedule();
  }
  document.querySelectorAll(".key[data-mode]").forEach((k) => k.addEventListener("click", () => setMode(k.dataset.mode)));
  window.addEventListener("hashchange", () => { const m = location.hash.slice(1); if (MODES.includes(m)) setMode(m); });
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, dialog")) return;
    const i = "12345".indexOf(e.key);
    if (i >= 0) setMode(MODES[i]);
  });
  function applyTemp() { setText("k-temp-t", S.fahrenheit ? "°F" : "°C"); document.querySelectorAll("#dash-gauges,#charts").forEach((b) => { b.dataset.keys = ""; }); schedule(); }
  $("k-temp").addEventListener("click", () => { S.fahrenheit = !S.fahrenheit; store.set("temp", S.fahrenheit ? "F" : "C"); applyTemp(); });
  function applyTheme() {
    document.documentElement.dataset.theme = S.theme;
    document.querySelector('meta[name="theme-color"]').content = S.theme === "light" ? "#e9ecee" : "#08090a";
    setText("k-theme-t", S.theme === "light" ? "Light" : "Dark");
    themeColours(); schedule();
  }
  $("k-theme").addEventListener("click", () => { S.theme = S.theme === "light" ? "dark" : "light"; store.set("theme", S.theme); applyTheme(); });
  document.querySelectorAll("[data-window]").forEach((k) => k.addEventListener("click", () => {
    S.window = Number(k.dataset.window); store.set("chartWindow", String(S.window)); schedule();
  }));
  $("k-chart-pause").addEventListener("click", () => {
    S.paused = !S.paused;
    $("k-chart-pause").classList.toggle("active", S.paused);
    $("k-chart-pause").querySelector("span").textContent = S.paused ? "Resume" : "Pause";
    schedule();
  });

  new ResizeObserver(schedule).observe(document.body);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) { pollStatus(); schedule(); } });

  applyTheme(); applyTemp(); setMode(S.mode);
  loadOnce(); pollStatus(); stream();
  setInterval(pollStatus, 2000);
  setInterval(trimHistory, 5000);
  setInterval(() => { if (S.mode === "charts" || S.mode === "engine") schedule(); }, 200);
  setInterval(schedule, 1000);
})();
