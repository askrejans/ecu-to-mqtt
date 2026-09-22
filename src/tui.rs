//! Terminal User Interface
//!
//! Provides a live TUI (using ratatui + crossterm) when the application is run
//! interactively (TTY detected).  In service/daemon mode the TUI is skipped and
//! structured logs are written to stdout instead.
//!
//! The dashboard works with every supported profile: Speeduino keeps its full
//! parameter panel, and all other ECUs render the canonical channels decoded
//! from their serial or CAN stream, with the same two-second freshness rule the
//! MQTT consumers use.
//!
//! # Layout
//! ```text
//! ┌───────────────────── ECU-to-MQTT ─────────────────────────┐
//! │ CONNECTIONS         │ ECU DATA                            │
//! │ ECU: ● ONLINE       │  RPM: 3000  MAP: 98 kPa             │
//! │ …                   │  …                                  │
//! ├─────────────────────────────────────────────────────────-─┤
//! │ LOG                                                       │
//! │ [INFO] Connected …                                        │
//! └───────────────────────────────────────────────────────────┘
//! ```

use crate::ecu_data_parser::SpeeduinoData;
use crate::ecu_protocol::{Channels, EcuProtocol};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap},
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

/// Channels older than this are shown as stale. Matches the expiry the
/// documented MQTT consumers apply to canonical samples.
const CHANNEL_STALE_AFTER: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// One decoded canonical channel and when it last changed.
#[derive(Debug, Clone)]
pub struct ChannelSample {
    pub value: f64,
    pub updated: Instant,
}

/// Shared state updated from the ECU/MQTT tasks and rendered by the TUI.
#[derive(Default)]
pub struct TuiState {
    pub ecu_connected: bool,
    pub mqtt_connected: bool,
    pub mqtt_enabled: bool,
    pub protocol: EcuProtocol,
    pub connection_address: String,
    pub mqtt_address: String,
    /// Full Speeduino parameter set (Speeduino profile only).
    pub ecu_data: Option<SpeeduinoData>,
    /// Canonical channels for every other profile.
    pub channels: BTreeMap<String, ChannelSample>,
    pub messages_published: u64,
    pub frames_decoded: u64,
}

impl TuiState {
    /// Merge one decoded packet. Channels a packet does not carry keep their
    /// previous value and age, exactly like the partial MQTT envelopes.
    pub fn update_channels(&mut self, channels: &Channels) {
        let now = Instant::now();
        for (name, value) in channels {
            self.channels.insert(
                name.clone(),
                ChannelSample {
                    value: *value,
                    updated: now,
                },
            );
        }
        self.frames_decoded = self.frames_decoded.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Log writer that captures tracing output into the TUI log panel
// ---------------------------------------------------------------------------

/// An `io::Write` implementation that appends formatted log lines to a
/// shared ring-buffer, which the TUI renders in the bottom log panel.
#[derive(Clone)]
pub struct TuiWriter {
    buffer: Arc<Mutex<VecDeque<String>>>,
}

impl TuiWriter {
    pub fn new(buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self { buffer }
    }
}

impl Write for TuiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(s) = std::str::from_utf8(buf) {
            let trimmed = s.trim_end_matches('\n');
            if !trimmed.is_empty() {
                let mut guard = self.buffer.lock().unwrap();
                if guard.len() >= 200 {
                    guard.pop_front();
                }
                guard.push_back(trimmed.to_string());
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TuiWriter {
    type Writer = TuiWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// TUI runner
// ---------------------------------------------------------------------------

/// Run the interactive TUI until the user presses `q` / `Ctrl+C`, or until
/// `cancel` fires (e.g. on SIGTERM).
///
/// Rendering happens every 100 ms; new ECU data and log entries are reflected
/// on the next frame.
pub async fn run_tui(
    state: Arc<RwLock<TuiState>>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = tui_loop(&mut terminal, state, log_buffer, cancel).await;

    // Restore terminal regardless of outcome
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<RwLock<TuiState>>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_tick = interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            _ = render_tick.tick() => {
                let s = state.read().await;
                let logs: Vec<String> = log_buffer.lock().unwrap().iter().cloned().collect();
                let snap = StateSnapshot {
                    ecu_connected: s.ecu_connected,
                    mqtt_connected: s.mqtt_connected,
                    mqtt_enabled: s.mqtt_enabled,
                    protocol: s.protocol,
                    connection_address: s.connection_address.clone(),
                    mqtt_address: s.mqtt_address.clone(),
                    ecu_data: s.ecu_data.clone(),
                    channels: s.channels.clone(),
                    messages_published: s.messages_published,
                    frames_decoded: s.frames_decoded,
                    logs,
                };
                drop(s);
                terminal.draw(|f| render(f, &snap))?;
            }

            Some(Ok(event)) = event_stream.next() => {
                if should_quit(&event) {
                    cancel.cancel();
                    break;
                }
            }
        }
    }
    Ok(())
}

fn should_quit(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(k)
            if k.code == KeyCode::Char('q')
            || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
    )
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StateSnapshot {
    ecu_connected: bool,
    mqtt_connected: bool,
    mqtt_enabled: bool,
    protocol: EcuProtocol,
    connection_address: String,
    mqtt_address: String,
    ecu_data: Option<SpeeduinoData>,
    channels: BTreeMap<String, ChannelSample>,
    messages_published: u64,
    frames_decoded: u64,
    logs: Vec<String>,
}

fn render(f: &mut Frame, snap: &StateSnapshot) {
    let area = f.area();

    // Vertical split: header | main | log
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(8),
        ])
        .split(area);

    let header_area = vertical[0];
    let main_area = vertical[1];
    let log_area = vertical[2];

    // Horizontal split: connections | data
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(1)])
        .split(main_area);

    let conn_area = horizontal[0];
    let data_area = horizontal[1];

    render_header(f, header_area, snap);
    render_connections(f, conn_area, snap);
    render_ecu_data(f, data_area, snap);
    render_log(f, log_area, snap);
}

fn render_header(f: &mut Frame, area: Rect, snap: &StateSnapshot) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            concat!(" ECU-to-MQTT v", env!("CARGO_PKG_VERSION"), " "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("│ "),
        Span::styled(
            snap.protocol.label(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ press "),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" to quit"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
}

fn status_indicator(connected: bool) -> Span<'static> {
    if connected {
        Span::styled("● ONLINE ", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ OFFLINE", Style::default().fg(Color::Red))
    }
}

fn render_connections(f: &mut Frame, area: Rect, snap: &StateSnapshot) {
    let mut lines: Vec<Line> = Vec::new();

    // Active profile
    lines.push(Line::from(vec![
        Span::styled("PROTO:", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {}", snap.protocol.name())),
    ]));
    lines.push(Line::default());

    // ECU connection
    lines.push(Line::from(vec![
        Span::styled("ECU:  ", Style::default().add_modifier(Modifier::BOLD)),
        status_indicator(snap.ecu_connected),
    ]));
    if !snap.connection_address.is_empty() {
        lines.push(Line::from(Span::raw(format!(
            "  {}",
            snap.connection_address
        ))));
    }
    lines.push(Line::default());

    // MQTT connection
    lines.push(Line::from(vec![
        Span::styled("MQTT: ", Style::default().add_modifier(Modifier::BOLD)),
        if snap.mqtt_enabled {
            status_indicator(snap.mqtt_connected)
        } else {
            Span::styled("DISABLED ", Style::default().fg(Color::DarkGray))
        },
    ]));
    if snap.mqtt_enabled && !snap.mqtt_address.is_empty() {
        lines.push(Line::from(Span::raw(format!("  {}", snap.mqtt_address))));
    }
    lines.push(Line::default());

    // Published count
    if snap.mqtt_enabled {
        lines.push(Line::from(vec![
            Span::styled("Msgs: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(snap.messages_published.to_string()),
        ]));
    }

    // Decoded packet count (profiles that feed the canonical channel panel)
    if snap.protocol != EcuProtocol::Speeduino {
        lines.push(Line::from(vec![
            Span::styled("Pkts: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(snap.frames_decoded.to_string()),
        ]));
    }

    let block = Block::default()
        .title(" CONNECTIONS ")
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1));
    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(para, area);
}

/// Canonical channel presentation: panel section, short label, unit and decimals.
/// Anything not listed is still shown, under OTHER, with its canonical name.
const CHANNEL_DISPLAY: &[(&str, &str, &str, &str, usize)] = &[
    ("ENGINE", "rpm", "RPM", "", 0),
    ("ENGINE", "throttle", "TPS", "%", 1),
    ("ENGINE", "manifoldKpa", "MAP", " kPa", 1),
    ("ENGINE", "boostKpa", "BST", " kPa", 1),
    ("ENGINE", "ignitionDeg", "ADV", "°", 1),
    ("FUEL", "lambda", "LAM", "", 3),
    ("FUEL", "lambda2", "LAM2", "", 3),
    ("FUEL", "afr", "AFR", "", 1),
    ("FUEL", "fuelPressureKpa", "FUEP", " kPa", 1),
    ("FUEL", "fuelC", "FUET", "°C", 1),
    ("TEMPERATURES", "coolantC", "CLT", "°C", 1),
    ("TEMPERATURES", "intakeC", "IAT", "°C", 1),
    ("TEMPERATURES", "oilC", "OILT", "°C", 1),
    ("TEMPERATURES", "transmissionC", "TRNT", "°C", 1),
    ("TEMPERATURES", "differentialC", "DIFT", "°C", 1),
    ("PRESSURE / ELECTRICAL", "oilPressureKpa", "OILP", " kPa", 1),
    (
        "PRESSURE / ELECTRICAL",
        "brakePressureKpa",
        "BRKP",
        " kPa",
        1,
    ),
    ("PRESSURE / ELECTRICAL", "batteryV", "BAT", "V", 1),
    ("VEHICLE", "ecuSpeedKmh", "SPD", " km/h", 1),
    ("VEHICLE", "wheelSpeedFlKmh", "WSFL", " km/h", 1),
    ("VEHICLE", "wheelSpeedFrKmh", "WSFR", " km/h", 1),
    ("VEHICLE", "wheelSpeedRlKmh", "WSRL", " km/h", 1),
    ("VEHICLE", "wheelSpeedRrKmh", "WSRR", " km/h", 1),
    ("VEHICLE", "gear", "GEAR", "", 0),
    ("VEHICLE", "brakeSwitch", "BRK", "", 0),
    ("VEHICLE", "clutchSwitch", "CLU", "", 0),
    ("VEHICLE", "lateralG", "LATG", " G", 2),
    ("VEHICLE", "longitudinalG", "LNGG", " G", 2),
];

/// Canonical channel dashboard used by every non-Speeduino profile. Channels
/// arrive in partial packets, so each one carries its own age and a channel the
/// ECU stopped broadcasting is dimmed rather than silently kept fresh.
fn render_channels(f: &mut Frame, area: Rect, snap: &StateSnapshot, block: Block) {
    if snap.channels.is_empty() {
        let para = Paragraph::new("Waiting for ECU data…")
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        f.render_widget(para, area);
        return;
    }

    let now = Instant::now();
    let lbl = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let sec = Style::default().fg(Color::DarkGray);
    let stale = Style::default().fg(Color::DarkGray);

    let inner_w = area.width.saturating_sub(4) as usize;
    let col_w = (inner_w / 3).max(14);

    let mut lines: Vec<Line> = Vec::new();
    let mut rendered: Vec<&str> = Vec::new();

    let push_cells = |lines: &mut Vec<Line>, cells: Vec<(String, String, bool)>| {
        for chunk in cells.chunks(3) {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (i, (label, value, fresh)) in chunk.iter().enumerate() {
                spans.push(Span::styled(format!("{:<5}", label), lbl));
                let text = if i + 1 < chunk.len() {
                    format!(": {:<width$}", value, width = col_w.saturating_sub(7))
                } else {
                    format!(": {}", value)
                };
                spans.push(Span::styled(
                    text,
                    if *fresh { Style::default() } else { stale },
                ));
            }
            lines.push(Line::from(spans));
        }
    };

    let mut sections: Vec<&str> = Vec::new();
    for (section, ..) in CHANNEL_DISPLAY {
        if !sections.contains(section) {
            sections.push(section);
        }
    }

    for section in sections {
        let cells: Vec<(String, String, bool)> = CHANNEL_DISPLAY
            .iter()
            .filter(|(s, ..)| s == &section)
            .filter_map(|(_, key, label, unit, decimals)| {
                let sample = snap.channels.get(*key)?;
                rendered.push(key);
                Some((
                    (*label).to_string(),
                    format!("{:.*}{}", decimals, sample.value, unit),
                    now.duration_since(sample.updated) < CHANNEL_STALE_AFTER,
                ))
            })
            .collect();
        if cells.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(format!("── {} ──", section), sec)));
        push_cells(&mut lines, cells);
    }

    let extra: Vec<(String, String, bool)> = snap
        .channels
        .iter()
        .filter(|(name, _)| !rendered.contains(&name.as_str()))
        .map(|(name, sample)| {
            (
                name.chars().take(5).collect::<String>(),
                format!("{:.2}", sample.value),
                now.duration_since(sample.updated) < CHANNEL_STALE_AFTER,
            )
        })
        .collect();
    if !extra.is_empty() {
        lines.push(Line::from(Span::styled("── OTHER ──", sec)));
        push_cells(&mut lines, extra);
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(
            "{} channels · dimmed = not refreshed for {}s",
            snap.channels.len(),
            CHANNEL_STALE_AFTER.as_secs()
        ),
        sec,
    )));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_ecu_data(f: &mut Frame, area: Rect, snap: &StateSnapshot) {
    let block = Block::default()
        .title(" ECU DATA ")
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1));

    if snap.protocol != EcuProtocol::Speeduino {
        render_channels(f, area, snap, block);
        return;
    }

    let Some(ref d) = snap.ecu_data else {
        let para = Paragraph::new("Waiting for ECU data…")
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        f.render_widget(para, area);
        return;
    };

    // ── Derived values ────────────────────────────────────────────────────
    // Gauge boost pressure: positive = boost, negative = vacuum
    let boost_rel = d.map as i32 - d.baro as i32;
    // Lambda from AFR target (afr_target stored ×10, stoich 14.7 → 147)
    let afr_lambda = d.afr_target as f32 / 147.0;
    // Dwell efficiency: actual measured vs requested dwell (%)
    let dwell_eff = d
        .actual_dwell
        .filter(|_| d.dwell > 0)
        .map(|ad| ad as f32 / d.dwell as f32 * 100.0);

    // ── Layout helpers ────────────────────────────────────────────────────
    // inner_w: usable width inside borders (2) + padding (2)
    let inner_w = area.width.saturating_sub(4) as usize;
    let col_w = (inner_w / 3).max(10);
    // Width reserved per value in non-last columns: label(4) + ": "(2) + val_w + "  "(2) = col_w
    let val_w = col_w.saturating_sub(8);

    let lbl = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let sec = Style::default().fg(Color::DarkGray);

    // Nested helpers (no closures needed — pure conversions)
    fn c(label: &str, value: String) -> (String, String) {
        (label.to_string(), value)
    }
    fn ms10(raw: u16) -> String {
        format!("{:.1}ms", raw as f32 / 10.0)
    }
    fn opt_ms10(o: Option<u16>) -> String {
        o.map_or_else(|| "—".into(), ms10)
    }
    fn opt_str<T: std::fmt::Display>(o: Option<T>) -> String {
        o.map_or_else(|| "—".into(), |v| v.to_string())
    }
    fn opt_unit<T: std::fmt::Display>(o: Option<T>, unit: &str) -> String {
        o.map_or_else(|| "—".into(), |v| format!("{}{}", v, unit))
    }

    let section_line = |title: &str| -> Line<'static> {
        Line::from(Span::styled(format!("── {} ──", title), sec))
    };

    let row = |cells: Vec<(String, String)>| -> Line<'static> {
        let n = cells.len();
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, (label, value)) in cells.into_iter().enumerate() {
            spans.push(Span::styled(format!("{:<4}", label), lbl));
            let part = if i + 1 < n {
                format!(": {:<width$}  ", value, width = val_w)
            } else {
                format!(": {}", value)
            };
            spans.push(Span::raw(part));
        }
        Line::from(spans)
    };

    let mut lines: Vec<Line> = Vec::new();

    // ── ENGINE ───────────────────────────────────────────────────────────
    lines.push(section_line("ENGINE"));
    lines.push(row(vec![
        c("RPM", d.rpm.to_string()),
        c("MAP", format!("{} kPa", d.map)),
        c("TPS", format!("{}%", d.tps)),
    ]));
    lines.push(row(vec![
        c("DRPM", format!("{:+}/s", d.rpm_dot)),
        c("DMAP", format!("{:.1} kPa/s", d.map_dot as f32 / 10.0)),
        c("DTPS", format!("{:.1}%/s", d.tps_dot as f32 / 10.0)),
    ]));
    lines.push(row(vec![
        c("ADV", format!("{}°", d.advance)),
        c("VE", format!("{}%", d.ve_current)),
        c(
            "AFR>",
            format!("{:.1} λ{:.2}", d.afr_target_real(), afr_lambda),
        ),
    ]));
    lines.push(row(vec![
        c("BARO", format!("{} kPa", d.baro)),
        c(
            "EMAP",
            d.emap.map_or_else(|| "—".into(), |v| format!("{} kPa", v)),
        ),
        c("GBST", format!("{:+} kPa", boost_rel)),
    ]));

    // ── SENSORS ──────────────────────────────────────────────────────────
    lines.push(section_line("SENSORS"));
    lines.push(row(vec![
        c("IAT", format!("{}°C", d.iat_celsius())),
        c("CLT", format!("{}°C", d.coolant_celsius())),
        c("FTMP", format!("{}°C", d.fuel_temp_celsius())),
    ]));
    lines.push(row(vec![
        c("BAT", format!("{:.1}V", d.battery_voltage())),
        c("O2P", d.o2_primary.to_string()),
        c("O2S", d.o2_secondary.to_string()),
    ]));

    // ── FUELING ──────────────────────────────────────────────────────────
    lines.push(section_line("FUELING"));
    lines.push(row(vec![
        c("PW1", ms10(d.pw1)),
        c("PW2", ms10(d.pw2)),
        c("PW3", ms10(d.pw3)),
    ]));
    lines.push(row(vec![
        c("PW4", ms10(d.pw4)),
        c("EGO", format!("{}%", d.ego_correction)),
        c("TAE", format!("{}%", d.tae_amount_pct())),
    ]));
    if d.pw5.is_some() || d.pw6.is_some() {
        lines.push(row(vec![
            c("PW5", opt_ms10(d.pw5)),
            c("PW6", opt_ms10(d.pw6)),
            c("PW7", opt_ms10(d.pw7)),
        ]));
        lines.push(row(vec![c("PW8", opt_ms10(d.pw8))]));
    }
    lines.push(row(vec![
        c("IATC", format!("{}%", d.iat_correction)),
        c("WUEC", format!("{}%", d.wue_correction)),
        c("BARC", format!("{}%", d.baro_correction)),
    ]));
    lines.push(row(vec![
        c("BATC", format!("{}%", d.bat_correction)),
        c("FTEC", format!("{}%", d.fuel_temp_correction)),
        c("CORR", format!("{}%", d.corrections)),
    ]));
    lines.push(row(vec![
        c("ASE", format!("{}%", d.ase_value)),
        c("FLXC", format!("{}%", d.flex_correction)),
    ]));

    // ── IGNITION ─────────────────────────────────────────────────────────
    lines.push(section_line("IGNITION"));
    lines.push(row(vec![
        c("DWL", ms10(d.dwell)),
        c("ADW", opt_ms10(d.actual_dwell)),
        c(
            "DEFF",
            dwell_eff.map_or_else(|| "—".into(), |e| format!("{:.0}%", e)),
        ),
    ]));
    lines.push(row(vec![
        c("ADV1", format!("{}°", d.advance1)),
        c("ADV2", format!("{}°", d.advance2)),
        c("KNK", opt_str(d.knock_count)),
    ]));
    lines.push(row(vec![c("KRET", opt_unit(d.knock_retard, "°"))]));

    // ── BOOST / VVT / FLEX ───────────────────────────────────────────────
    lines.push(section_line("BOOST / VVT / FLEX"));
    lines.push(row(vec![
        // boost_duty_raw stores duty as 0-100 (= actual %)
        c("BTGT", format!("{} kPa", d.boost_target_kpa())),
        c("BDUT", format!("{}%", d.boost_duty_raw)),
        c("ETH", format!("{}%", d.ethanol_pct)),
    ]));
    lines.push(row(vec![
        c("VVT1", format!("{}°", d.vvt1_angle)),
        c("VT1T", format!("{}°", d.vvt1_target_angle)),
        c("VT1D", format!("{}%", d.vvt1_duty)),
    ]));
    lines.push(row(vec![
        c("VVT2", format!("{}°", d.vvt2_angle)),
        c("VT2T", format!("{}°", d.vvt2_target_angle)),
        c("VT2D", format!("{}%", d.vvt2_duty)),
    ]));
    lines.push(row(vec![
        c("FLXI", format!("{}°", d.flex_ign_correction)),
        c("FLXB", d.flex_boost_correction.to_string()),
    ]));

    // ── VEHICLE ──────────────────────────────────────────────────────────
    lines.push(section_line("VEHICLE"));
    lines.push(row(vec![
        c("VSS", format!("{} km/h", d.vss)),
        c(
            "GEAR",
            if d.gear == 0 {
                "N".into()
            } else {
                d.gear.to_string()
            },
        ),
        c("WMI", format!("{} µs", d.wmi_pw)),
    ]));
    lines.push(row(vec![
        c("OIL", format!("{} kPa", d.oil_pressure)),
        c("FPRS", format!("{} kPa", d.fuel_pressure)),
    ]));
    lines.push(row(vec![
        c("FAN", opt_unit(d.fan_duty, "%")),
        c(
            "ACS",
            d.air_con_status
                .map_or_else(|| "—".into(), |v| format!("{:#04x}", v)),
        ),
    ]));

    // ── SYSTEM ───────────────────────────────────────────────────────────
    lines.push(section_line("SYSTEM"));
    lines.push(row(vec![
        c("LPS", d.loops_per_second.to_string()),
        c("RAM", format!("{} B", d.free_ram)),
        c("SECL", d.secl.to_string()),
    ]));
    lines.push(row(vec![
        c("SYNC", d.sync_loss_counter.to_string()),
        c("ERR", format!("{:#04x}", d.next_error)),
        c("SDCS", format!("{:#04x}", d.ts_sd_status)),
    ]));
    lines.push(row(vec![
        c("LOAD", d.fuel_load.to_string()),
        c("IGLD", d.ign_load.to_string()),
        c("CILT", d.cl_idle_target.to_string()),
    ]));
    lines.push(row(vec![
        c("STS1", format!("{:#04x}", d.status1)),
        c("ENG", format!("{:#04x}", d.engine)),
        c("SPRK", format!("{:#04x}", d.spark)),
    ]));
    lines.push(row(vec![
        c("STS3", format!("{:#04x}", d.status3)),
        c("STS4", format!("{:#04x}", d.status4)),
        c("STS5", opt_str(d.status5.map(|v| format!("{:#04x}", v)))),
    ]));

    // ── CAN INPUTS (only shown when any channel is non-zero) ─────────────
    if d.canin.iter().any(|&v| v != 0) {
        lines.push(section_line("CAN INPUTS"));
        for i in (0..16usize).step_by(3) {
            let end = (i + 3).min(16);
            let cells: Vec<(String, String)> = (i..end)
                .map(|j| (format!("C{:<2}", j), d.canin[j].to_string()))
                .collect();
            lines.push(row(cells));
        }
    }

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn render_log(f: &mut Frame, area: Rect, snap: &StateSnapshot) {
    let max_lines = (area.height.saturating_sub(2)) as usize;
    let start = snap.logs.len().saturating_sub(max_lines);
    let items: Vec<ListItem> = snap.logs[start..]
        .iter()
        .map(|line| {
            let style = if line.contains("ERROR") {
                Style::default().fg(Color::Red)
            } else if line.contains("WARN") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(Line::from(Span::styled(line.clone(), style)))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" LOG (most recent) ")
            .borders(Borders::ALL),
    );
    f.render_widget(list, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn draw(snap: &StateSnapshot) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(96, 44)).unwrap();
        terminal.draw(|f| render(f, snap)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| row(buffer, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Column of `needle` in `line`, counted in cells rather than bytes: rows
    /// contain box-drawing and degree characters that are several bytes wide.
    fn column_of(line: &str, needle: &str) -> Option<usize> {
        let cells: Vec<char> = line.chars().collect();
        let wanted: Vec<char> = needle.chars().collect();
        cells
            .windows(wanted.len())
            .position(|window| window == wanted.as_slice())
    }

    /// Style of the first cell of `value` on the row that contains `label`.
    fn value_style(buffer: &Buffer, label: &str, value: &str) -> Style {
        for y in 0..buffer.area.height {
            let line = row(buffer, y);
            if let (Some(_), Some(at)) = (column_of(&line, label), column_of(&line, value)) {
                return buffer[(at as u16, y)].style();
            }
        }
        panic!(
            "no row contains both {label} and {value}:\n{}",
            text(buffer)
        );
    }

    fn sample(value: f64, age: Duration) -> ChannelSample {
        ChannelSample {
            value,
            updated: Instant::now().checked_sub(age).unwrap(),
        }
    }

    fn can_snapshot() -> StateSnapshot {
        StateSnapshot {
            protocol: EcuProtocol::HaltechCanV2,
            ecu_connected: true,
            connection_address: "TCP 127.0.0.1:29536".into(),
            channels: BTreeMap::from([
                ("rpm".into(), sample(6000., Duration::ZERO)),
                ("coolantC".into(), sample(90.4, Duration::ZERO)),
                ("lambda".into(), sample(0.887, Duration::ZERO)),
                ("gear".into(), sample(3., Duration::ZERO)),
                ("wheelSpeedFlKmh".into(), sample(112.5, Duration::ZERO)),
            ]),
            frames_decoded: 42,
            ..Default::default()
        }
    }

    #[test]
    fn header_and_connections_name_the_active_profile() {
        let screen = text(&draw(&can_snapshot()));
        assert!(screen.contains("ECU-to-MQTT"), "{screen}");
        assert!(screen.contains("Haltech CAN V2"), "{screen}");
        assert!(screen.contains("haltech_can_v2"), "{screen}");
        assert!(screen.contains("127.0.0.1:29536"), "{screen}");
        // Decoded packet counter replaces the Speeduino-only parameter panel.
        assert!(screen.contains("Pkts: 42"), "{screen}");
    }

    #[test]
    fn canonical_panel_shows_decoded_channels_with_units() {
        let screen = text(&draw(&can_snapshot()));
        for expected in [
            "RPM",
            "6000",
            "CLT",
            "90.4°C",
            "LAM",
            "0.887",
            "GEAR",
            "3",
            "WSFL",
            "112.5 km/h",
            "5 channels",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected} in:\n{screen}"
            );
        }
    }

    #[test]
    fn channels_outside_the_known_table_are_still_displayed() {
        let mut snap = can_snapshot();
        snap.channels
            .insert("customThing".into(), sample(12.5, Duration::ZERO));
        let screen = text(&draw(&snap));
        assert!(screen.contains("OTHER"), "{screen}");
        assert!(screen.contains("custo"), "{screen}");
    }

    #[test]
    fn channels_the_ecu_stopped_broadcasting_are_dimmed() {
        let mut snap = can_snapshot();
        snap.channels
            .insert("rpm".into(), sample(6000., Duration::from_secs(5)));
        let buffer = draw(&snap);
        assert_eq!(
            value_style(&buffer, "RPM", "6000").fg,
            Some(Color::DarkGray)
        );
        assert_ne!(
            value_style(&buffer, "CLT", "90.4").fg,
            Some(Color::DarkGray)
        );
    }

    #[test]
    fn every_profile_renders_without_panicking() {
        for protocol in EcuProtocol::ALL {
            let snap = StateSnapshot {
                protocol,
                ..can_snapshot()
            };
            let screen = text(&draw(&snap));
            assert!(screen.contains(protocol.name()), "{screen}");
        }
    }

    #[test]
    fn speeduino_keeps_its_full_parameter_panel() {
        let snap = StateSnapshot {
            protocol: EcuProtocol::Speeduino,
            ecu_connected: true,
            ecu_data: Some(SpeeduinoData {
                rpm: 3000,
                map: 98,
                ..Default::default()
            }),
            ..Default::default()
        };
        let screen = text(&draw(&snap));
        for expected in ["ENGINE", "FUELING", "IGNITION", "VEHICLE", "SYSTEM", "3000"] {
            assert!(
                screen.contains(expected),
                "missing {expected} in:\n{screen}"
            );
        }
    }

    #[test]
    fn both_panels_wait_for_the_first_packet() {
        for protocol in [EcuProtocol::Speeduino, EcuProtocol::MotecM1Pdm] {
            let snap = StateSnapshot {
                protocol,
                ..Default::default()
            };
            assert!(text(&draw(&snap)).contains("Waiting for ECU data"));
        }
    }

    #[test]
    fn partial_packets_merge_instead_of_replacing_the_channel_set() {
        let mut state = TuiState::default();
        state.update_channels(&Channels::from([("rpm".into(), 6000.)]));
        state.update_channels(&Channels::from([("coolantC".into(), 90.)]));
        state.update_channels(&Channels::from([("rpm".into(), 6100.)]));
        assert_eq!(state.frames_decoded, 3);
        assert_eq!(state.channels["rpm"].value, 6100.);
        assert_eq!(state.channels["coolantC"].value, 90.);
    }
}
