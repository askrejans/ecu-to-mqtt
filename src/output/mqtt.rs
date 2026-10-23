//! MQTT output (rumqttc, rustls).
//!
//! | Format          | Topic(s)                               | Retained | Rate            |
//! |-----------------|----------------------------------------|----------|-----------------|
//! | `canonical`     | `<base>/telemetry` (G86 envelope)      | never    | every sample    |
//! | `legacy`        | `<base>/RPM`, `<base>/CLT` … (Speeduino) | no     | on change ≤ `mqtt_max_rate_hz`, all every 1 s |
//! | `json`          | `<base>/state`                         | yes      | ≤ `mqtt_max_rate_hz` |
//! | `homeassistant` | `<prefix>/sensor/<node>/…/config`      | yes      | on connect      |
//!
//! `<base>/status` is `online`/`offline` (retained; `offline` is the last will).
//! `<base>/health` (retained) is the G86 car-computer bridge health JSON
//! (`crate::health`): every 5 s and at once when its state or link changes;
//! a final `down` document precedes `offline` on a clean shutdown.
//!
//! **On demand** (for consumers on slow links, such as the G86 display): a
//! message on `<base>/telemetry/get` whose payload is a reply topic is
//! answered with one snapshot of every fresh channel on that topic. The
//! consumer asks again when it has used the answer, so it receives as much
//! as it can take and never builds a backlog. No answer while the ECU link
//! is down or the data is stale.
//! Publishing never blocks acquisition: while the broker is unreachable live
//! samples are dropped and counted, and state is re-sent after reconnecting.

use crate::config::{AppConfig, MqttFormat};
use crate::health;
use crate::protocol::speeduino::{self, SpeeduinoData};
use crate::sample::Stamped;
use crate::shared::{CHANNEL_MAX_AGE_MS, Counters, LinkStatus, Shared};
use anyhow::{Context, Result};
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, Transport};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

const RECONNECT_MAX: Duration = Duration::from_secs(60);
/// Legacy topics are not retained (a stale RPM must not outlive the engine),
/// so unchanged values are re-sent this often for subscribers that join late.
const LEGACY_REFRESH: Duration = Duration::from_secs(1);

/// Requests per reply topic are answered at most this often.
/// Per reply topic: bursts of up to DEMAND_BURST requests are answered at
/// once (a display keeps several in flight, and Wi-Fi delivers them in bursts),
/// at most DEMAND_RATE per second on average.
const DEMAND_BURST: f32 = 12.0;
const DEMAND_RATE: f32 = 100.0;
/// A request waits at most this long for a new sample.
const DEMAND_HOLD: Duration = Duration::from_secs(2);
const DEMAND_MAX_CLIENTS: usize = 32;

/// Token bucket for on-demand answers to one reply topic.
#[derive(Debug, Clone, Copy)]
pub struct DemandBucket {
    tokens: f32,
    at: Instant,
}

impl DemandBucket {
    pub fn new(now: Instant) -> Self {
        DemandBucket {
            tokens: DEMAND_BURST,
            at: now,
        }
    }

    /// One answer if the bucket has a token; refills at DEMAND_RATE per second.
    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.at).as_secs_f32();
        self.tokens = (self.tokens + dt * DEMAND_RATE).min(DEMAND_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One display asking on demand: its rate bucket and the requests it holds
/// open. A request is answered with the next sample it hasn't had yet, so
/// a display gets every new sample once, as soon as it exists, and never a
/// duplicate (that would only cost airtime on a weak link).
#[derive(Debug, Clone, Copy)]
pub struct DemandClient {
    bucket: DemandBucket,
    pending: u8,
    last_seq: Option<u64>,
    since: Instant,
}

impl DemandClient {
    pub fn new(now: Instant) -> Self {
        DemandClient {
            bucket: DemandBucket::new(now),
            pending: 0,
            last_seq: None,
            since: now,
        }
    }

    /// A request arrived; false if over the rate (dropped).
    pub fn request(&mut self, now: Instant) -> bool {
        if !self.bucket.take(now) {
            return false;
        }
        self.pending = (self.pending + 1).min(DEMAND_BURST as u8);
        self.since = now;
        true
    }

    /// Whether to answer one held request now with sample `seq` (None: no
    /// fresh sample). Held requests expire after DEMAND_HOLD.
    pub fn ready(&mut self, seq: Option<u64>, now: Instant) -> bool {
        if self.pending == 0 {
            return false;
        }
        if now.saturating_duration_since(self.since) > DEMAND_HOLD {
            self.pending = 0;
            return false;
        }
        match seq {
            Some(s) if self.last_seq != Some(s) => {
                self.last_seq = Some(s);
                self.pending -= 1;
                true
            }
            _ => false,
        }
    }

    pub fn last_seen(&self) -> Instant {
        self.since
    }
}

/// A usable reply topic: 1–128 bytes of UTF-8, no wildcards, not a `$` system topic.
pub fn valid_reply_topic(payload: &[u8]) -> Option<&str> {
    let t = std::str::from_utf8(payload).ok()?.trim();
    (!t.is_empty() && t.len() <= 128 && !t.contains(['+', '#', '\0']) && !t.starts_with('$'))
        .then_some(t)
}

/// The sample an on-demand answer would carry now, or None (no fresh data).
pub fn demand_sequence(shared: &Shared) -> Option<u64> {
    let s = shared.read();
    if !s.link.is_up() || s.sample_age_s(Instant::now())? * 1000.0 > CHANNEL_MAX_AGE_MS as f64 {
        return None;
    }
    Some(s.last_sequence)
}

/// Snapshot of every fresh channel, shaped like the canonical envelope
/// (`partial: false`, all channels). `None` when the ECU is not delivering.
pub fn demand_reply(shared: &Shared) -> Option<String> {
    let now = Instant::now();
    let s = shared.read();
    if !s.link.is_up() || s.sample_age_s(now)? * 1000.0 > CHANNEL_MAX_AGE_MS as f64 {
        return None;
    }
    let mut captured = 0u64;
    let channels: serde_json::Map<String, serde_json::Value> = s
        .fresh_channels(now)
        .map(|(k, c)| {
            captured = captured.max(c.captured_ms);
            (k.clone(), json!(c.value))
        })
        .collect();
    if channels.is_empty() {
        return None;
    }
    Some(
        json!({
            "schema": 1,
            "source": s.protocol.name(),
            "partial": false,
            "timestampMs": captured,
            "sequence": s.last_sequence,
            "channels": channels,
        })
        .to_string(),
    )
}

/// `base` + `sub` with exactly one slash between them.
pub fn topic(base: &str, sub: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        sub.trim_start_matches('/')
    )
}

fn qos(level: i32) -> QoS {
    match level {
        1 => QoS::AtLeastOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtMostOnce,
    }
}

pub fn client_id(config: &AppConfig) -> String {
    config.mqtt_client_id.clone().unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        format!("ecu-to-mqtt-{:x}{:04x}", std::process::id(), nanos & 0xffff)
    })
}

fn options(config: &AppConfig, id: &str) -> Result<MqttOptions> {
    let mut o = MqttOptions::new(id, &config.mqtt_host, config.mqtt_port);
    o.set_keep_alive(Duration::from_secs(30));
    o.set_clean_session(true);
    o.set_request_channel_capacity(config.message_buffer_size.max(16));
    o.set_max_packet_size(256 * 1024, 256 * 1024);
    o.set_last_will(LastWill::new(
        topic(&config.base_topic(), "status"),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    if let Some(user) = &config.mqtt_username {
        o.set_credentials(user, config.mqtt_password()?.unwrap_or_default());
    }
    if config.mqtt_use_tls || config.mqtt_ca_cert_path.is_some() {
        let transport = match &config.mqtt_ca_cert_path {
            Some(ca) => {
                let ca = std::fs::read(ca).with_context(|| format!("reading {ca}"))?;
                let client_auth =
                    match (&config.mqtt_client_cert_path, &config.mqtt_client_key_path) {
                        (Some(c), Some(k)) => Some((
                            std::fs::read(c).with_context(|| format!("reading {c}"))?,
                            std::fs::read(k).with_context(|| format!("reading {k}"))?,
                        )),
                        _ => None,
                    };
                Transport::tls(ca, client_auth, None)
            }
            None => Transport::tls_with_default_config(),
        };
        o.set_transport(transport);
    }
    Ok(o)
}

/// The `<base>/state` document.
pub fn state_document(stamped: &Stamped, shared: &Shared) -> serde_json::Value {
    let s = shared.read();
    let now = Instant::now();
    let channels: serde_json::Map<String, serde_json::Value> = s
        .fresh_channels(now)
        .map(|(k, c)| (k.clone(), json!(c.value)))
        .collect();
    let mut doc = json!({
        "timestampMs": stamped.sample.captured_ms,
        "sequence": stamped.sequence,
        "source": stamped.sample.source,
        "protocol": s.protocol.name(),
        "firmware": s.firmware,
        "channels": channels,
    });
    if let Some(d) = &stamped.sample.speeduino {
        doc["speeduino"] = serde_json::to_value(d.as_ref()).unwrap_or_default();
        doc["flags"] = json!(active_flags(d));
    }
    doc
}

/// Names of the status bits that are set, e.g. `engine.running`.
pub fn active_flags(d: &SpeeduinoData) -> Vec<String> {
    speeduino::flags(d)
        .into_iter()
        .filter(|f| f.on)
        .map(|f| format!("{}.{}", f.group, f.name))
        .collect()
}

/// Home Assistant discovery documents for the main channels.
pub fn homeassistant_discovery(config: &AppConfig) -> Vec<(String, String)> {
    let base = config.base_topic();
    let base = base.trim_end_matches('/');
    let node = {
        let raw = config
            .mqtt_client_id
            .clone()
            .unwrap_or_else(|| base.to_string());
        let n: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let n = n.trim_matches('_').to_string();
        if n.is_empty() {
            "ecu_to_mqtt".into()
        } else {
            n
        }
    };
    let state = topic(base, "state");
    let avail = topic(base, "status");
    let prefix = config.mqtt_homeassistant_prefix.trim_end_matches('/');
    let sensors: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        ("rpm", "Engine speed", Some("rpm"), None),
        (
            "coolantC",
            "Coolant temperature",
            Some("°C"),
            Some("temperature"),
        ),
        (
            "intakeC",
            "Intake temperature",
            Some("°C"),
            Some("temperature"),
        ),
        ("batteryV", "Battery voltage", Some("V"), Some("voltage")),
        (
            "manifoldKpa",
            "Manifold pressure",
            Some("kPa"),
            Some("pressure"),
        ),
        ("boostKpa", "Boost", Some("kPa"), Some("pressure")),
        ("throttle", "Throttle", Some("%"), None),
        ("afr", "Air/fuel ratio", None, None),
        ("lambda", "Lambda", None, None),
        ("ignitionDeg", "Ignition advance", Some("°"), None),
        (
            "oilPressureKpa",
            "Oil pressure",
            Some("kPa"),
            Some("pressure"),
        ),
        ("ecuSpeedKmh", "Vehicle speed", Some("km/h"), Some("speed")),
    ];
    sensors
        .iter()
        .map(|(key, name, unit, class)| {
            let mut doc = json!({
                "name": name,
                "unique_id": format!("{node}_{key}"),
                "state_topic": state,
                "value_template": format!("{{{{ value_json.channels.{key} | default(none) }}}}"),
                "availability_topic": avail,
                "expire_after": 10,
                "device": {
                    "identifiers": [format!("ecu_to_mqtt_{node}")],
                    "name": format!("ECU {base}"),
                    "manufacturer": "ecu-to-mqtt",
                    "model": "ecu-to-mqtt",
                    "sw_version": env!("CARGO_PKG_VERSION"),
                },
            });
            let o = doc.as_object_mut().expect("object");
            if let Some(u) = unit {
                o.insert("unit_of_measurement".into(), json!(u));
                o.insert("state_class".into(), json!("measurement"));
            }
            if let Some(c) = class {
                o.insert("device_class".into(), json!(c));
            }
            (
                format!("{prefix}/sensor/{node}/{}/config", key.to_lowercase()),
                doc.to_string(),
            )
        })
        .collect()
}

pub async fn run(
    config: Arc<AppConfig>,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<()> {
    let id = client_id(&config);
    let (client, mut eventloop) =
        AsyncClient::new(options(&config, &id)?, config.message_buffer_size.max(16));
    let (conn_tx, conn_rx) = watch::channel(false);
    info!(
        "MQTT client {id} → {}:{}",
        config.mqtt_host, config.mqtt_port
    );

    let el_shared = shared.clone();
    let el_cancel = cancel.clone();
    let el_client = client.clone();
    let demand_topic = topic(&config.base_topic(), "telemetry/get");
    let event_loop = tokio::spawn(async move {
        let mut delay = Duration::from_secs(1);
        let mut answered: HashMap<String, DemandClient> = HashMap::new();
        // releases held requests when a new sample exists
        let mut tick = tokio::time::interval(Duration::from_millis(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        el_shared.set_mqtt(LinkStatus::Connecting);
        loop {
            let polled = tokio::select! {
                r = eventloop.poll() => r,
                _ = tick.tick(), if answered.values().any(|c| c.pending > 0) => {
                    let now = Instant::now();
                    let seq = demand_sequence(&el_shared);
                    for (reply, c) in answered.iter_mut() {
                        if c.ready(seq, now)
                            && let Some(doc) = demand_reply(&el_shared)
                        {
                            match el_client.try_publish(reply.clone(), QoS::AtMostOnce, false, doc) {
                                Ok(()) => Counters::inc(&el_shared.counters.mqtt_published),
                                Err(_) => Counters::inc(&el_shared.counters.mqtt_dropped),
                            }
                        }
                    }
                    continue;
                }
                _ = el_cancel.cancelled() => {
                    let _ = tokio::time::timeout(Duration::from_secs(1), async {
                        while eventloop.poll().await.is_ok() {}
                    }).await;
                    break;
                }
            };
            match polled {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    info!("MQTT connected");
                    let _ = el_client.try_subscribe(demand_topic.clone(), QoS::AtMostOnce);
                    delay = Duration::from_secs(1);
                    let _ = conn_tx.send(true);
                    el_shared.set_mqtt(LinkStatus::Connected);
                }
                Ok(Event::Incoming(Packet::Publish(p))) if p.topic == demand_topic => {
                    let Some(reply) = valid_reply_topic(&p.payload) else {
                        continue;
                    };
                    let now = Instant::now();
                    if answered.len() >= DEMAND_MAX_CLIENTS && !answered.contains_key(reply) {
                        answered.retain(|_, c| {
                            now.duration_since(c.last_seen()) < Duration::from_secs(10)
                        });
                        if answered.len() >= DEMAND_MAX_CLIENTS {
                            continue;
                        }
                    }
                    let c = answered
                        .entry(reply.to_string())
                        .or_insert_with(|| DemandClient::new(now));
                    if !c.request(now) {
                        continue;
                    }
                    // answered at once if there is a sample it hasn't had, else held for the next one
                    if c.ready(demand_sequence(&el_shared), now)
                        && let Some(doc) = demand_reply(&el_shared)
                    {
                        match el_client.try_publish(reply, QoS::AtMostOnce, false, doc) {
                            Ok(()) => Counters::inc(&el_shared.counters.mqtt_published),
                            Err(_) => Counters::inc(&el_shared.counters.mqtt_dropped),
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = conn_tx.send(false);
                    el_shared.set_mqtt(LinkStatus::Disconnected);
                    warn!("MQTT connection error: {e}; retrying in {delay:?}");
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = el_cancel.cancelled() => break,
                    }
                    delay = (delay * 2).min(RECONNECT_MAX);
                    el_shared.set_mqtt(LinkStatus::Connecting);
                }
            }
        }
        el_shared.set_mqtt(LinkStatus::Disconnected);
    });

    let mut publisher = Publisher::new(config, client, shared.clone());
    publisher.run(shared.bus.subscribe(), conn_rx, cancel).await;
    if let Err(e) = event_loop.await {
        error!("MQTT event loop: {e}");
    }
    Ok(())
}

struct Publisher {
    config: Arc<AppConfig>,
    client: AsyncClient,
    shared: Arc<Shared>,
    qos: QoS,
    last: HashMap<&'static str, String>,
    last_refresh: Option<Instant>,
    min_interval: Option<Duration>,
    last_state_at: Option<Instant>,
    latest: Option<Arc<Stamped>>,
    health: health::Tracker,
    /// (state, link) and time of the last published health document.
    health_sent: Option<(String, String, Instant)>,
}

impl Publisher {
    fn new(config: Arc<AppConfig>, client: AsyncClient, shared: Arc<Shared>) -> Self {
        let min_interval = (config.mqtt_max_rate_hz > 0.0)
            .then(|| Duration::from_secs_f64(1.0 / config.mqtt_max_rate_hz));
        Self {
            qos: qos(config.mqtt_qos),
            config,
            client,
            shared,
            last: HashMap::new(),
            last_refresh: None,
            min_interval,
            last_state_at: None,
            latest: None,
            health: health::Tracker::new(),
            health_sent: None,
        }
    }

    fn send(&self, topic: String, payload: String, retain: bool, qos: QoS) -> bool {
        match self.client.try_publish(topic, qos, retain, payload) {
            Ok(()) => {
                Counters::inc(&self.shared.counters.mqtt_published);
                true
            }
            Err(_) => {
                Counters::inc(&self.shared.counters.mqtt_dropped);
                false
            }
        }
    }

    fn on_connect(&mut self) {
        self.last.clear();
        self.last_state_at = None;
        let base = self.config.base_topic();
        self.send(
            topic(&base, "status"),
            "online".into(),
            true,
            QoS::AtLeastOnce,
        );
        if self.config.wants(MqttFormat::Homeassistant) {
            for (t, doc) in homeassistant_discovery(&self.config) {
                self.send(t, doc, true, QoS::AtLeastOnce);
            }
        }
        self.health_sent = None;
        self.publish_health(true);
        if let Some(latest) = self.latest.clone() {
            self.publish_state(&latest, true);
        }
    }

    /// `<base>/health`: when forced, every HEALTH_INTERVAL, or when the state
    /// or link changed since the last document.
    fn publish_health(&mut self, connected: bool) {
        if !connected {
            return;
        }
        let now = Instant::now();
        let doc = self
            .health
            .document(&self.shared, now, health::clock_synced());
        let key = (
            doc["state"].as_str().unwrap_or_default().to_string(),
            doc["link"].as_str().unwrap_or_default().to_string(),
        );
        let due = match &self.health_sent {
            None => true,
            Some((state, link, at)) => {
                *state != key.0
                    || *link != key.1
                    || now.duration_since(*at) >= health::HEALTH_INTERVAL
            }
        };
        if due
            && self.send(
                topic(&self.config.base_topic(), "health"),
                doc.to_string(),
                true,
                QoS::AtLeastOnce,
            )
        {
            self.health_sent = Some((key.0, key.1, now));
        }
    }

    /// Legacy scalars and the JSON state document, rate-limited.
    fn publish_state(&mut self, stamped: &Stamped, force: bool) {
        let now = Instant::now();
        if !force
            && let (Some(min), Some(last)) = (self.min_interval, self.last_state_at)
            && now.duration_since(last) < min
        {
            return;
        }
        self.last_state_at = Some(now);
        let base = self.config.base_topic();
        if self.config.wants(MqttFormat::Legacy)
            && let Some(d) = &stamped.sample.speeduino
        {
            if self
                .last_refresh
                .is_none_or(|t| now.duration_since(t) >= LEGACY_REFRESH)
            {
                self.last.clear();
                self.last_refresh = Some(now);
            }
            for (code, value) in speeduino::get_params_to_publish(d) {
                if self.last.get(code) == Some(&value) {
                    continue;
                }
                if self.send(topic(&base, code), value.clone(), false, self.qos) {
                    self.last.insert(code, value);
                }
            }
        }
        if self.config.wants(MqttFormat::Json) || self.config.wants(MqttFormat::Homeassistant) {
            let doc = state_document(stamped, &self.shared).to_string();
            self.send(topic(&base, "state"), doc, true, self.qos);
        }
    }

    fn on_sample(&mut self, stamped: Arc<Stamped>, connected: bool) {
        self.latest = Some(stamped.clone());
        self.health.on_sample(
            stamped.sample.source,
            stamped.sample.captured_ms,
            Instant::now(),
        );
        if !connected {
            Counters::inc(&self.shared.counters.mqtt_dropped);
            return;
        }
        if self.config.wants(MqttFormat::Canonical) {
            // Never retained: consumers reject replayed frames as stale.
            let t = topic(&self.config.base_topic(), "telemetry");
            self.send(t, stamped.envelope.to_string(), false, self.qos);
        }
        self.publish_state(&stamped, false);
    }

    async fn run(
        &mut self,
        mut bus: broadcast::Receiver<Arc<Stamped>>,
        mut conn: watch::Receiver<bool>,
        cancel: CancellationToken,
    ) {
        let mut connected = false;
        // state changes (link lost, samples stopping) are noticed within a second
        let mut health_tick = tokio::time::interval(Duration::from_secs(1));
        health_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = health_tick.tick() => self.publish_health(connected),
                changed = conn.changed() => {
                    if changed.is_err() { break; }
                    connected = *conn.borrow_and_update();
                    if connected { self.on_connect(); }
                }
                event = bus.recv() => match event {
                    Ok(stamped) => self.on_sample(stamped, connected),
                    Err(broadcast::error::RecvError::Lagged(n)) => Counters::add(&self.shared.counters.mqtt_dropped, n),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        if connected {
            let last = self
                .health
                .document(&self.shared, Instant::now(), health::clock_synced());
            self.send(
                topic(&self.config.base_topic(), "health"),
                health::shutdown_document(last).to_string(),
                true,
                QoS::AtLeastOnce,
            );
            self.send(
                topic(&self.config.base_topic(), "status"),
                "offline".into(),
                true,
                QoS::AtLeastOnce,
            );
            let _ = self.client.try_disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Channels, EcuProtocol};
    use crate::sample::Sample;
    use crate::shared::Info;

    #[test]
    fn demand_client_answers_each_sample_once() {
        let t0 = Instant::now();
        let mut c = DemandClient::new(t0);
        for _ in 0..3 {
            assert!(c.request(t0));
        }
        assert!(c.ready(Some(1), t0), "a sample it hasn't had: at once");
        assert!(!c.ready(Some(1), t0), "the same sample again: held");
        assert!(!c.ready(None, t0), "no fresh data: held");
        assert!(
            c.ready(Some(2), t0 + Duration::from_millis(20)),
            "the next sample releases one"
        );
        assert!(c.ready(Some(3), t0 + Duration::from_millis(40)));
        assert!(
            !c.ready(Some(4), t0 + Duration::from_millis(60)),
            "all three answered"
        );
        // held requests expire
        assert!(c.request(t0 + Duration::from_millis(100)));
        assert!(!c.ready(
            Some(4),
            t0 + Duration::from_millis(100) + DEMAND_HOLD + Duration::from_millis(1)
        ));
        assert!(!c.ready(Some(5), t0 + Duration::from_secs(10)));
    }

    #[test]
    fn demand_bucket_allows_bursts_and_caps_the_rate() {
        let t0 = Instant::now();
        let mut b = DemandBucket::new(t0);
        // a burst of in-flight requests arriving together is answered in full
        for _ in 0..12 {
            assert!(b.take(t0));
        }
        assert!(!b.take(t0), "beyond the burst");
        // refills at 100 per second: one more after 10 ms
        assert!(b.take(t0 + Duration::from_millis(10)));
        assert!(!b.take(t0 + Duration::from_millis(11)));
        // a second of requests every millisecond: about 100 answered
        let mut n = 0;
        for ms in 0..1000u64 {
            if b.take(t0 + Duration::from_millis(20 + ms)) {
                n += 1;
            }
        }
        assert!((95..=110).contains(&n), "{n} answered");
    }

    #[test]
    fn demand_reply_topics_are_validated() {
        assert_eq!(
            valid_reply_topic(b"/GOLF86/DISPLAY/g86-display-2b84/ecu"),
            Some("/GOLF86/DISPLAY/g86-display-2b84/ecu")
        );
        assert_eq!(valid_reply_topic(b" a/b \n"), Some("a/b"));
        for bad in [
            &b""[..],
            b"a/+/b",
            b"a/#",
            b"$SYS/x",
            &[0xff, 0xfe],
            &[b'a'; 129],
        ] {
            assert_eq!(valid_reply_topic(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn demand_reply_needs_fresh_data_and_merges_channels() {
        use crate::shared::ChannelState;
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, true);
        assert_eq!(
            demand_reply(&shared),
            None,
            "no answer before the first sample"
        );
        shared.set_link(LinkStatus::Connected, "test");
        {
            let mut s = shared.write();
            s.last_sample_at = Some(Instant::now());
            for (k, v) in [("rpm", 3000.0), ("coolantC", 88.0)] {
                s.channels.insert(
                    k.into(),
                    ChannelState {
                        value: v,
                        captured_ms: 7,
                        source: "test",
                        at: Instant::now(),
                    },
                );
            }
        }
        let doc: serde_json::Value = serde_json::from_str(&demand_reply(&shared).unwrap()).unwrap();
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["partial"], false);
        assert_eq!(doc["channels"]["rpm"], 3000.0);
        assert_eq!(doc["channels"]["coolantC"], 88.0);
        shared.write().last_sample_at = Some(Instant::now() - Duration::from_secs(5));
        assert_eq!(demand_reply(&shared), None, "stale data is not answered");
    }

    #[test]
    fn topics_join_with_one_slash() {
        assert_eq!(topic("/ECU/", "RPM"), "/ECU/RPM");
        assert_eq!(topic("/GOLF86/ECU", "/telemetry"), "/GOLF86/ECU/telemetry");
    }

    #[test]
    fn qos_levels_map() {
        assert_eq!(qos(0), QoS::AtMostOnce);
        assert_eq!(qos(1), QoS::AtLeastOnce);
        assert_eq!(qos(2), QoS::ExactlyOnce);
    }

    #[test]
    fn state_document_includes_detail_and_active_flags() {
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, true);
        let packet = speeduino::encode(&speeduino::SimValues {
            rpm: 3000,
            engine: 1,
            status2: 0x80,
            ..Default::default()
        });
        let d = speeduino::parse(&packet).unwrap();
        let mut sample = Sample::new("speeduino-primary-a", false, 5, speeduino::canonical(&d));
        sample.speeduino = Some(Arc::new(d));
        shared.write().channels.insert(
            "rpm".into(),
            crate::shared::ChannelState {
                value: 3000.0,
                captured_ms: 5,
                source: "x",
                at: Instant::now(),
            },
        );
        let stamped = Stamped {
            sequence: 9,
            sample,
            envelope: Arc::from("{}"),
        };
        let doc = state_document(&stamped, &shared);
        assert_eq!(doc["sequence"], 9);
        assert_eq!(doc["channels"]["rpm"], 3000.0);
        assert_eq!(doc["speeduino"]["rpm"], 3000);
        assert_eq!(doc["flags"], json!(["engine.running", "status2.sync"]));
        let _ = Channels::new();
    }

    #[tokio::test]
    async fn legacy_topics_send_changes_and_refresh_unchanged_values() {
        let config = Arc::new(AppConfig {
            mqtt_formats: vec![MqttFormat::Legacy],
            mqtt_max_rate_hz: 0.0,
            ..AppConfig::default()
        });
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, true);
        let (client, _eventloop) = AsyncClient::new(options(&config, "t").unwrap(), 1024);
        let mut p = Publisher::new(config, client, shared.clone());
        let stamped = |rpm| {
            let d = speeduino::parse(&speeduino::encode(&speeduino::SimValues {
                rpm,
                ..Default::default()
            }))
            .unwrap();
            let mut sample = Sample::new("speeduino-primary-a", false, 5, Channels::new());
            sample.speeduino = Some(Arc::new(d));
            Stamped {
                sequence: 0,
                sample,
                envelope: Arc::from("{}"),
            }
        };
        let published = || Counters::get(&shared.counters.mqtt_published);

        p.publish_state(&stamped(3000), false);
        let all = published();
        assert!(all > 80, "first sample publishes every topic ({all})");
        p.publish_state(&stamped(3000), false);
        assert_eq!(published(), all, "unchanged values are suppressed");
        p.publish_state(&stamped(3100), false);
        assert_eq!(published(), all + 1, "only RPM changed");
        p.last_refresh = Some(Instant::now() - LEGACY_REFRESH);
        p.publish_state(&stamped(3100), false);
        assert_eq!(
            published(),
            2 * all + 1,
            "periodic refresh re-sends everything"
        );
    }

    #[test]
    fn homeassistant_documents_read_the_state_topic() {
        let config = AppConfig {
            mqtt_base_topic: "/CAR/ECU/".into(),
            ..AppConfig::default()
        };
        let docs = homeassistant_discovery(&config);
        assert_eq!(docs[0].0, "homeassistant/sensor/car_ecu/rpm/config");
        let v: serde_json::Value = serde_json::from_str(&docs[0].1).unwrap();
        assert_eq!(v["state_topic"], "/CAR/ECU/state");
        assert_eq!(v["availability_topic"], "/CAR/ECU/status");
        assert_eq!(
            v["value_template"],
            "{{ value_json.channels.rpm | default(none) }}"
        );
        assert!(docs.iter().any(|(t, _)| t.ends_with("/coolantc/config")));
    }
}
