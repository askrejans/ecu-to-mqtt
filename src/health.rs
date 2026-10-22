//! Bridge health for the G86 car computer (`<base>/health`, retained).
//!
//! Contract: `contracts/CAR_COMPUTER.md` §2 of the G86 API. Published every
//! [`HEALTH_INTERVAL`] and at once when `state` or `link` changes, so the
//! logger, the phone and the pit wall see a dead ECU link within seconds
//! without parsing telemetry.
//!
//! ```json
//! {"schema":1,"bridge":"ecu-to-mqtt","version":"0.7.4","bootId":"…",
//!  "timestampMs":1792664130000,"state":"ok","link":"connected",
//!  "sources":["speeduino-primary-a"],"rateHz":49.8,"ageMs":14,
//!  "channels":["coolantC","rpm"],"errors":{"decode":0,"timeouts":2,"reconnects":0},
//!  "clockSynced":true}
//! ```

use crate::sample::now_ms;
use crate::shared::{Counters, LinkStatus, Shared};
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

pub const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
/// `rateHz` window.
const RATE_WINDOW: Duration = Duration::from_secs(5);
/// Samples older than this make the bridge `degraded`.
const STALE: Duration = Duration::from_secs(2);
/// No sample for this long (or no link) makes it `down`.
const DEAD: Duration = Duration::from_secs(10);
/// An error counter that increased within this window makes it `degraded`.
const ERROR_WINDOW: Duration = Duration::from_secs(5);

/// `ok` / `degraded` / `down` from the link, the age of the last sample and
/// whether errors are rising.
pub fn state(link_up: bool, sample_age: Option<Duration>, errors_rising: bool) -> &'static str {
    match sample_age {
        _ if !link_up => "down",
        None => "down",
        Some(age) if age > DEAD => "down",
        Some(age) if age > STALE => "degraded",
        Some(_) if errors_rising => "degraded",
        Some(_) => "ok",
    }
}

pub fn link_name(link: LinkStatus) -> &'static str {
    match link {
        LinkStatus::Connected => "connected",
        LinkStatus::Connecting => "connecting",
        LinkStatus::Disconnected | LinkStatus::Disabled => "disconnected",
    }
}

/// Rolling state behind the health document: published-sample times,
/// sources seen, and the last time an error counter moved.
pub struct Tracker {
    published: VecDeque<Instant>,
    /// First sample ever: the rate is averaged over less than the window
    /// right after start, so the first documents don't under-report.
    first: Option<Instant>,
    sources: BTreeSet<&'static str>,
    last_captured_ms: Option<u64>,
    errors_total: u64,
    errors_rose_at: Option<Instant>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            published: VecDeque::new(),
            first: None,
            sources: BTreeSet::new(),
            last_captured_ms: None,
            errors_total: 0,
            errors_rose_at: None,
        }
    }

    /// A canonical sample went out.
    pub fn on_sample(&mut self, source: &'static str, captured_ms: u64, now: Instant) {
        self.first.get_or_insert(now);
        self.published.push_back(now);
        self.prune(now);
        self.sources.insert(source);
        self.last_captured_ms = Some(captured_ms);
    }

    fn prune(&mut self, now: Instant) {
        while self
            .published
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > RATE_WINDOW)
        {
            self.published.pop_front();
        }
    }

    fn rate_hz(&mut self, now: Instant) -> f64 {
        self.prune(now);
        let n = self.published.len() as f64;
        let span = self.first.map_or(RATE_WINDOW, |t| {
            now.saturating_duration_since(t)
                .clamp(Duration::from_secs(1), RATE_WINDOW)
        });
        (n / span.as_secs_f64() * 10.0).round() / 10.0
    }

    /// The health document now. `boot_id` is the telemetry envelope's.
    pub fn document(&mut self, shared: &Shared, now: Instant, clock_synced: bool) -> Value {
        let c = &shared.counters;
        let (decode, timeouts, reconnects) = (
            Counters::get(&c.decode_errors),
            Counters::get(&c.timeouts),
            Counters::get(&c.reconnects),
        );
        let total = decode + timeouts + reconnects;
        if total > self.errors_total {
            self.errors_total = total;
            self.errors_rose_at = Some(now);
        }
        let rising = self
            .errors_rose_at
            .is_some_and(|t| now.saturating_duration_since(t) <= ERROR_WINDOW);
        let rate = self.rate_hz(now);
        let s = shared.read();
        let age = s.sample_age_s(now).map(Duration::from_secs_f64);
        let channels: Vec<&String> = s.fresh_channels(now).map(|(k, _)| k).collect();
        let wall = now_ms();
        json!({
            "schema": 1,
            "bridge": "ecu-to-mqtt",
            "version": env!("CARGO_PKG_VERSION"),
            "bootId": shared.boot_id,
            "timestampMs": wall,
            "state": state(s.link.is_up(), age, rising),
            "link": link_name(s.link),
            "sources": self.sources.iter().collect::<Vec<_>>(),
            "rateHz": rate,
            "ageMs": self.last_captured_ms.map(|t| wall.saturating_sub(t)),
            "channels": channels,
            "errors": {"decode": decode, "timeouts": timeouts, "reconnects": reconnects},
            "clockSynced": clock_synced,
        })
    }
}

/// The document published on graceful shutdown, before `status` = `offline`.
pub fn shutdown_document(mut doc: Value) -> Value {
    doc["state"] = "down".into();
    doc["link"] = "disconnected".into();
    doc["rateHz"] = 0.0.into();
    doc["channels"] = json!([]);
    doc["timestampMs"] = now_ms().into();
    doc
}

/// Whether the system clock is NTP/GPS-disciplined.
#[cfg(target_os = "linux")]
pub fn clock_synced() -> bool {
    // SAFETY: adjtimex with modes = 0 only reads the kernel clock state into
    // a zeroed, correctly sized struct.
    let mut t: libc::timex = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::adjtimex(&mut t) };
    r != libc::TIME_ERROR && (t.status & libc::STA_UNSYNC) == 0
}

/// Other platforms manage their own clock; assume it is synchronised.
#[cfg(not(target_os = "linux"))]
pub fn clock_synced() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EcuProtocol;
    use crate::shared::{ChannelState, Info};

    #[test]
    fn state_follows_link_age_and_errors() {
        let s = |secs: f64| Some(Duration::from_secs_f64(secs));
        assert_eq!(state(false, s(0.1), false), "down");
        assert_eq!(state(true, None, false), "down");
        assert_eq!(state(true, s(0.1), false), "ok");
        assert_eq!(state(true, s(0.1), true), "degraded");
        assert_eq!(state(true, s(3.0), false), "degraded");
        assert_eq!(state(true, s(11.0), false), "down");
    }

    #[test]
    fn document_matches_the_contract() {
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, true);
        let mut t = Tracker::new();
        let now = Instant::now();
        let doc = t.document(&shared, now, true);
        assert_eq!(doc["state"], "down");
        assert_eq!(doc["link"], "connecting");
        assert!(doc["ageMs"].is_null());

        shared.set_link(LinkStatus::Connected, "sim");
        {
            let mut s = shared.write();
            s.last_sample_at = Some(now);
            s.channels.insert(
                "rpm".into(),
                ChannelState {
                    value: 6120.0,
                    captured_ms: now_ms(),
                    source: "speeduino-primary-a",
                    at: now,
                },
            );
        }
        for i in 0..25 {
            t.on_sample(
                "speeduino-primary-a",
                now_ms(),
                now - Duration::from_millis(200 * (25 - i)),
            );
        }
        let doc = t.document(&shared, now, false);
        let keys: BTreeSet<&str> = doc
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "schema",
                "bridge",
                "version",
                "bootId",
                "timestampMs",
                "state",
                "link",
                "sources",
                "rateHz",
                "ageMs",
                "channels",
                "errors",
                "clockSynced"
            ])
        );
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["bridge"], "ecu-to-mqtt");
        assert_eq!(doc["bootId"], shared.boot_id.as_str());
        assert_eq!(doc["state"], "ok");
        assert_eq!(doc["link"], "connected");
        assert_eq!(doc["sources"], json!(["speeduino-primary-a"]));
        assert_eq!(doc["rateHz"], 5.0);
        assert!(doc["ageMs"].as_u64().unwrap() < 1000);
        assert_eq!(doc["channels"], json!(["rpm"]));
        assert_eq!(doc["clockSynced"], false);

        // a new decode error degrades the bridge for the error window
        Counters::inc(&shared.counters.decode_errors);
        let doc = t.document(&shared, now, true);
        assert_eq!(doc["state"], "degraded");
        assert_eq!(doc["errors"]["decode"], 1);
        assert_eq!(
            t.document(&shared, now + Duration::from_secs(6), true)["state"],
            "degraded",
            "sample is now 6 s old"
        );

        let down = shutdown_document(doc);
        assert_eq!(down["state"], "down");
        assert_eq!(down["link"], "disconnected");
    }
}
