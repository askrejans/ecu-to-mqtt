//! HTTP API: health probe, Prometheus metrics, REST snapshot, live SSE and the
//! web dashboard.
//!
//! | Route                  | Auth  | Description                                        |
//! |------------------------|-------|----------------------------------------------------|
//! | `GET /health`          | open  | 200 when the ECU link is up and data is fresh, else 503 |
//! | `GET /metrics`         | open  | Prometheus text exposition                         |
//! | `GET /api/v1/status`   | token | wiring, link, counters                             |
//! | `GET /api/v1/live`     | token | channels with ages, Speeduino detail, status flags |
//! | `GET /api/v1/channels` | token | channel catalogue (label, unit, range, group)      |
//! | `GET /api/v1/config`   | token | effective configuration, secrets masked            |
//! | `GET /api/v1/stream`   | token | Server-Sent Events `sample`, ≤ 25 Hz per client    |

use crate::config::AppConfig;
use crate::protocol::speeduino;
use crate::sample::Stamped;
use crate::shared::{Counters, LinkStatus, Shared};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response, sse},
    routing::get,
};
use futures_util::stream::{self, Stream};
use serde_json::json;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Minimum spacing of SSE samples per browser connection.
const SSE_MIN_INTERVAL: Duration = Duration::from_millis(40);

/// (name, label, unit, group, min, max) — gauge ranges for dashboards.
pub const CATALOG: &[(&str, &str, &str, &str, f64, f64)] = &[
    ("rpm", "Engine speed", "rpm", "engine", 0.0, 9000.0),
    ("throttle", "Throttle", "%", "engine", 0.0, 100.0),
    (
        "manifoldKpa",
        "Manifold pressure",
        "kPa",
        "engine",
        0.0,
        250.0,
    ),
    ("boostKpa", "Boost", "kPa", "engine", -100.0, 200.0),
    ("baroKpa", "Barometer", "kPa", "engine", 50.0, 120.0),
    (
        "ignitionDeg",
        "Ignition advance",
        "°",
        "ignition",
        -10.0,
        50.0,
    ),
    ("dwellMs", "Dwell", "ms", "ignition", 0.0, 10.0),
    ("afr", "Air/fuel ratio", "", "fuel", 10.0, 20.0),
    ("afrTarget", "AFR target", "", "fuel", 10.0, 20.0),
    ("lambda", "Lambda", "λ", "fuel", 0.6, 1.4),
    ("lambda2", "Lambda 2", "λ", "fuel", 0.6, 1.4),
    ("veCurrent", "VE", "%", "fuel", 0.0, 150.0),
    (
        "injectorPwMs",
        "Injector pulse width",
        "ms",
        "fuel",
        0.0,
        25.0,
    ),
    (
        "fuelPressureKpa",
        "Fuel pressure",
        "kPa",
        "fuel",
        0.0,
        600.0,
    ),
    ("coolantC", "Coolant", "°C", "temperature", -20.0, 130.0),
    ("intakeC", "Intake air", "°C", "temperature", -20.0, 80.0),
    ("oilC", "Oil temperature", "°C", "temperature", -20.0, 150.0),
    (
        "fuelC",
        "Fuel temperature",
        "°C",
        "temperature",
        -20.0,
        80.0,
    ),
    (
        "transmissionC",
        "Transmission",
        "°C",
        "temperature",
        -20.0,
        150.0,
    ),
    (
        "differentialC",
        "Differential",
        "°C",
        "temperature",
        -20.0,
        150.0,
    ),
    (
        "oilPressureKpa",
        "Oil pressure",
        "kPa",
        "engine",
        0.0,
        800.0,
    ),
    ("batteryV", "Battery", "V", "electrical", 8.0, 16.0),
    (
        "ecuSpeedKmh",
        "Vehicle speed",
        "km/h",
        "vehicle",
        0.0,
        300.0,
    ),
    ("gear", "Gear", "", "vehicle", -1.0, 8.0),
    ("lateralG", "Lateral g", "g", "vehicle", -2.0, 2.0),
    ("longitudinalG", "Longitudinal g", "g", "vehicle", -2.0, 2.0),
    (
        "brakePressureKpa",
        "Brake pressure",
        "kPa",
        "vehicle",
        0.0,
        10000.0,
    ),
    ("brakeSwitch", "Brake switch", "", "vehicle", 0.0, 1.0),
    ("clutchSwitch", "Clutch switch", "", "vehicle", 0.0, 1.0),
    (
        "wheelSpeedFlKmh",
        "Wheel speed FL",
        "km/h",
        "vehicle",
        0.0,
        300.0,
    ),
    (
        "wheelSpeedFrKmh",
        "Wheel speed FR",
        "km/h",
        "vehicle",
        0.0,
        300.0,
    ),
    (
        "wheelSpeedRlKmh",
        "Wheel speed RL",
        "km/h",
        "vehicle",
        0.0,
        300.0,
    ),
    (
        "wheelSpeedRrKmh",
        "Wheel speed RR",
        "km/h",
        "vehicle",
        0.0,
        300.0,
    ),
    (
        "syncLossCount",
        "Sync losses",
        "",
        "diagnostics",
        0.0,
        255.0,
    ),
];

#[derive(Clone)]
struct Ctx {
    shared: Arc<Shared>,
    token: Option<Arc<str>>,
    config: Arc<serde_json::Value>,
}

#[derive(Default)]
pub struct Options {
    pub token: Option<String>,
    pub cors_origin: Option<String>,
    pub ui: bool,
    pub config: serde_json::Value,
}

impl Options {
    pub fn from_config(c: &AppConfig) -> Self {
        Self {
            token: c.http_token.clone(),
            cors_origin: c.http_cors_origin.clone(),
            ui: c.http_ui,
            config: c.redacted(),
        }
    }
}

pub fn router(shared: Arc<Shared>, opts: Options) -> Router {
    let Options {
        token,
        cors_origin,
        ui,
        config,
    } = opts;
    let ctx = Ctx {
        shared,
        token: token.filter(|t| !t.is_empty()).map(Arc::from),
        config: Arc::new(config),
    };
    let api = Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/live", get(live))
        .route("/api/v1/channels", get(channels))
        .route("/api/v1/config", get(config_handler))
        .route("/api/v1/stream", get(stream_events))
        .route_layer(middleware::from_fn_with_state(ctx.clone(), require_token));
    let cors = cors_origin.and_then(|o| HeaderValue::from_str(&o).ok());
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .merge(api);
    if ui {
        app = app.merge(super::web::router());
    }
    app.layer(middleware::from_fn(move |req: Request, next: Next| {
        let cors = cors.clone();
        async move {
            let mut res = next.run(req).await;
            let h = res.headers_mut();
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(origin) = cors {
                h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static("authorization"),
                );
            }
            res
        }
    }))
    .with_state(ctx)
}

pub async fn run(
    config: Arc<AppConfig>,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<()> {
    let addr: std::net::SocketAddr = format!("{}:{}", config.http_bind, config.http_port)
        .parse()
        .context("invalid http_bind/http_port")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding HTTP server to {addr}"))?;
    if config.http_token.is_none() && !addr.ip().is_loopback() {
        tracing::warn!(
            "HTTP API on {addr} has no http_token; /api is readable by anyone on the network"
        );
    }
    info!("HTTP API listening on http://{addr}");
    if config.http_ui {
        info!("Web dashboard on http://{addr}/");
    }
    axum::serve(listener, router(shared, Options::from_config(&config)))
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await?;
    Ok(())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    if let Some(expected) = &ctx.token {
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !ct_eq(presented.as_bytes(), expected.as_bytes()) {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
                Json(json!({"error": "unauthorized"})),
            )
                .into_response();
        }
    }
    next.run(req).await
}

async fn health(State(ctx): State<Ctx>) -> Response {
    let h = ctx.shared.health();
    let code = if h.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(h)).into_response()
}

fn counters(shared: &Shared) -> serde_json::Value {
    let c = &shared.counters;
    json!({
        "samples": Counters::get(&c.samples),
        "decode_errors": Counters::get(&c.decode_errors),
        "timeouts": Counters::get(&c.timeouts),
        "reconnects": Counters::get(&c.reconnects),
        "mqtt_published": Counters::get(&c.mqtt_published),
        "mqtt_dropped": Counters::get(&c.mqtt_dropped),
        "archive_errors": Counters::get(&c.archive_errors),
    })
}

async fn status(State(ctx): State<Ctx>) -> Response {
    Json(json!({"info": ctx.shared.info, "health": ctx.shared.health(), "counters": counters(&ctx.shared)})).into_response()
}

/// Everything a dashboard needs for one frame.
pub fn live_document(shared: &Shared) -> serde_json::Value {
    let now = Instant::now();
    let s = shared.read();
    let channels: serde_json::Map<String, serde_json::Value> = s
        .channels
        .iter()
        .map(|(k, c)| {
            (k.clone(), json!({"value": c.value, "ageMs": now.saturating_duration_since(c.at).as_millis() as u64, "source": c.source}))
        })
        .collect();
    let mut doc = json!({
        "protocol": s.protocol.name(),
        "protocolLabel": s.protocol.label(),
        "firmware": s.firmware,
        "link": s.link,
        "linkDetail": s.link_detail,
        "sequence": s.last_sequence,
        "sampleAgeS": s.sample_age_s(now),
        "sampleRateHz": s.sample_rate,
        "pollLatencyMs": s.poll_latency_ms,
        "channels": channels,
    });
    if let Some(d) = &s.speeduino {
        doc["speeduino"] = serde_json::to_value(d.as_ref()).unwrap_or_default();
        doc["flags"] = serde_json::to_value(speeduino::flags(d)).unwrap_or_default();
        doc["squirtsPerCycle"] = json!(speeduino::squirts_per_cycle(d));
    }
    doc
}

async fn live(State(ctx): State<Ctx>) -> Response {
    Json(live_document(&ctx.shared)).into_response()
}

async fn channels(State(_): State<Ctx>) -> Response {
    let list: Vec<serde_json::Value> = CATALOG
        .iter()
        .map(|(name, label, unit, group, min, max)| json!({"name": name, "label": label, "unit": unit, "group": group, "min": min, "max": max}))
        .collect();
    Json(list).into_response()
}

async fn config_handler(State(ctx): State<Ctx>) -> Response {
    Json(ctx.config.as_ref().clone()).into_response()
}

fn sse_payload(stamped: &Stamped) -> serde_json::Value {
    let mut v = json!({
        "sequence": stamped.sequence,
        "timestampMs": stamped.sample.captured_ms,
        "source": stamped.sample.source,
        "partial": stamped.sample.partial,
        "channels": stamped.sample.channels,
    });
    if let Some(d) = &stamped.sample.speeduino {
        v["speeduino"] = serde_json::to_value(d.as_ref()).unwrap_or_default();
        v["flags"] = serde_json::to_value(speeduino::flags(d)).unwrap_or_default();
    }
    v
}

async fn stream_events(
    State(ctx): State<Ctx>,
) -> sse::Sse<impl Stream<Item = std::result::Result<sse::Event, Infallible>>> {
    let rx = ctx.shared.bus.subscribe();
    let events = stream::unfold((rx, None::<Instant>), |(mut rx, mut last)| async move {
        loop {
            match rx.recv().await {
                Ok(stamped) => {
                    // Partial CAN samples always pass; complete packets are thinned
                    // so a browser is not flooded at the ECU poll rate.
                    let now = Instant::now();
                    if !stamped.sample.partial
                        && last.is_some_and(|t| now.duration_since(t) < SSE_MIN_INTERVAL)
                    {
                        continue;
                    }
                    last = Some(now);
                    if let Ok(e) = sse::Event::default()
                        .event("sample")
                        .json_data(sse_payload(&stamped))
                    {
                        return Some((Ok(e), (rx, last)));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    sse::Sse::new(events).keep_alive(sse::KeepAlive::new().interval(Duration::from_secs(15)))
}

// ── Prometheus ───────────────────────────────────────────────────────────

fn metric(out: &mut String, name: &str, kind: &str, help: &str, value: impl std::fmt::Display) {
    let _ = writeln!(
        out,
        "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}"
    );
}

pub fn render_metrics(shared: &Shared) -> String {
    let c = &shared.counters;
    let now = Instant::now();
    let s = shared.read();
    let mut o = String::with_capacity(4096);
    let _ = writeln!(
        o,
        "# HELP ecu_to_mqtt_build_info Build information\n# TYPE ecu_to_mqtt_build_info gauge\necu_to_mqtt_build_info{{version=\"{}\",protocol=\"{}\"}} 1",
        shared.info.version,
        s.protocol.name()
    );
    metric(
        &mut o,
        "ecu_connected",
        "gauge",
        "1 if the ECU link is up",
        i32::from(s.link == LinkStatus::Connected),
    );
    metric(
        &mut o,
        "ecu_samples_total",
        "counter",
        "Decoded samples",
        Counters::get(&c.samples),
    );
    metric(
        &mut o,
        "ecu_decode_errors_total",
        "counter",
        "Replies or frames that could not be decoded",
        Counters::get(&c.decode_errors),
    );
    metric(
        &mut o,
        "ecu_timeouts_total",
        "counter",
        "Polls without a reply",
        Counters::get(&c.timeouts),
    );
    metric(
        &mut o,
        "ecu_reconnects_total",
        "counter",
        "ECU link reconnections",
        Counters::get(&c.reconnects),
    );
    metric(
        &mut o,
        "ecu_sample_rate_hz",
        "gauge",
        "Samples per second",
        s.sample_rate,
    );
    metric(
        &mut o,
        "ecu_sample_age_seconds",
        "gauge",
        "Seconds since the last sample",
        s.sample_age_s(now).unwrap_or(f64::NAN),
    );
    metric(
        &mut o,
        "ecu_poll_latency_ms",
        "gauge",
        "Request to complete reply, serial ECUs",
        s.poll_latency_ms.unwrap_or(f64::NAN),
    );
    let _ = writeln!(
        o,
        "# HELP ecu_channel Latest fresh value of each canonical channel\n# TYPE ecu_channel gauge"
    );
    for (name, ch) in s.fresh_channels(now) {
        let _ = writeln!(o, "ecu_channel{{name=\"{name}\"}} {}", ch.value);
    }
    metric(
        &mut o,
        "mqtt_connected",
        "gauge",
        "1 if the MQTT broker connection is up",
        i32::from(s.mqtt == LinkStatus::Connected),
    );
    metric(
        &mut o,
        "mqtt_messages_published_total",
        "counter",
        "MQTT messages handed to the client",
        Counters::get(&c.mqtt_published),
    );
    metric(
        &mut o,
        "mqtt_messages_dropped_total",
        "counter",
        "Live MQTT messages dropped",
        Counters::get(&c.mqtt_dropped),
    );
    metric(
        &mut o,
        "archive_errors_total",
        "counter",
        "Archive packets that could not be written",
        Counters::get(&c.archive_errors),
    );
    o
}

async fn metrics(State(ctx): State<Ctx>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        render_metrics(&ctx.shared),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EcuProtocol;
    use crate::shared::{ChannelState, Info};
    use axum::body::Body;
    use tower::ServiceExt;

    fn opts(token: Option<&str>) -> Options {
        Options {
            token: token.map(String::from),
            cors_origin: None,
            ui: true,
            config: AppConfig::default().redacted(),
        }
    }

    fn shared_live() -> Arc<Shared> {
        let shared = Shared::new(
            Info {
                version: "test",
                ..Info::default()
            },
            EcuProtocol::Speeduino,
            false,
        );
        shared.set_link(LinkStatus::Connected, "/dev/test @ 115200 baud");
        let d = speeduino::parse(&speeduino::encode(&speeduino::SimValues {
            rpm: 2500,
            engine: 1,
            ..Default::default()
        }))
        .unwrap();
        let mut s = shared.write();
        s.last_sample_at = Some(Instant::now());
        s.channels.insert(
            "rpm".into(),
            ChannelState {
                value: 2500.0,
                captured_ms: 1,
                source: "speeduino-primary-a",
                at: Instant::now(),
            },
        );
        s.speeduino = Some(Arc::new(d));
        drop(s);
        shared
    }

    async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String) {
        let mut req = Request::builder().uri(path);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn health_metrics_open_api_needs_token() {
        let app = router(shared_live(), opts(Some("t")));
        let (code, body) = get(&app, "/health", None).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        let (_, body) = get(&app, "/metrics", None).await;
        assert!(body.contains("ecu_connected 1"));
        assert!(body.contains("ecu_channel{name=\"rpm\"} 2500"));
        assert_eq!(
            get(&app, "/api/v1/live", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (code, body) = get(&app, "/api/v1/live", Some("t")).await;
        assert_eq!(code, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["channels"]["rpm"]["value"], 2500.0);
        assert_eq!(v["speeduino"]["rpm"], 2500);
        assert!(
            v["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["name"] == "running" && f["on"] == true)
        );
    }

    #[tokio::test]
    async fn catalog_config_and_unhealthy_state() {
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, false);
        let app = router(shared, opts(None));
        assert_eq!(
            get(&app, "/health", None).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let (_, body) = get(&app, "/api/v1/channels", None).await;
        assert!(body.contains("\"name\":\"rpm\""));
        let (_, body) = get(&app, "/api/v1/config", None).await;
        assert!(body.contains("\"ecu_protocol\":\"auto\""));
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
