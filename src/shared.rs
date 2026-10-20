//! Process-wide state shared by the hub, outputs, HTTP API and TUI.
//!
//! * Bus — lossy broadcast of stamped samples. A slow consumer lags and skips;
//!   it never back-pressures ECU acquisition.
//! * [`Snapshot`] — latest values for request/response readers. Guarded by a
//!   std `RwLock` that is never held across `.await`.
//! * [`Counters`] — lock-free statistics for Prometheus and the dashboards.

use crate::protocol::EcuProtocol;
use crate::protocol::speeduino::SpeeduinoData;
use crate::sample::Stamped;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;
use tokio::sync::broadcast;

/// Channels older than this are stale (matches the G86 consumer's 2 s window).
pub const CHANNEL_MAX_AGE_MS: u64 = 2000;
const BUS_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStatus {
    Disabled,
    Connecting,
    Connected,
    Disconnected,
}

impl LinkStatus {
    pub fn is_up(self) -> bool {
        self == Self::Connected
    }
}

#[derive(Default)]
pub struct Counters {
    pub samples: AtomicU64,
    pub decode_errors: AtomicU64,
    pub timeouts: AtomicU64,
    pub reconnects: AtomicU64,
    pub mqtt_published: AtomicU64,
    pub mqtt_dropped: AtomicU64,
    pub archive_errors: AtomicU64,
}

impl Counters {
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// Static description of how the process is wired.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Info {
    pub version: &'static str,
    pub profile: String,
    pub connection: String,
    pub mqtt: Option<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelState {
    pub value: f64,
    pub captured_ms: u64,
    pub source: &'static str,
    #[serde(skip)]
    pub at: Instant,
}

pub struct Snapshot {
    pub link: LinkStatus,
    /// Resolved endpoint, e.g. `/dev/ttyUSB0 @ 115200 baud`.
    pub link_detail: String,
    pub link_since: Instant,
    pub last_sample_at: Option<Instant>,
    /// ECU signature (`Q` reply), e.g. `speeduino 202501`.
    pub firmware: Option<String>,
    /// Profile in use (resolved when configured as `auto`).
    pub protocol: EcuProtocol,
    pub channels: BTreeMap<String, ChannelState>,
    pub speeduino: Option<Arc<SpeeduinoData>>,
    pub mqtt: LinkStatus,
    /// Samples per second over the last second.
    pub sample_rate: f64,
    /// Request → complete response time of the last poll (serial ECUs).
    pub poll_latency_ms: Option<f64>,
    pub last_sequence: u64,
}

impl Snapshot {
    fn new(protocol: EcuProtocol, mqtt_enabled: bool) -> Self {
        Self {
            link: LinkStatus::Connecting,
            link_detail: String::new(),
            link_since: Instant::now(),
            last_sample_at: None,
            firmware: None,
            protocol,
            channels: BTreeMap::new(),
            speeduino: None,
            mqtt: if mqtt_enabled {
                LinkStatus::Connecting
            } else {
                LinkStatus::Disabled
            },
            sample_rate: 0.0,
            poll_latency_ms: None,
            last_sequence: 0,
        }
    }

    pub fn sample_age_s(&self, now: Instant) -> Option<f64> {
        self.last_sample_at
            .map(|t| now.saturating_duration_since(t).as_secs_f64())
    }

    /// Channels updated within [`CHANNEL_MAX_AGE_MS`].
    pub fn fresh_channels(&self, now: Instant) -> impl Iterator<Item = (&String, &ChannelState)> {
        self.channels.iter().filter(move |(_, c)| {
            now.saturating_duration_since(c.at).as_millis() as u64 <= CHANNEL_MAX_AGE_MS
        })
    }
}

pub struct Shared {
    pub info: Info,
    pub counters: Counters,
    pub bus: broadcast::Sender<Arc<Stamped>>,
    snapshot: RwLock<Snapshot>,
    pub started: Instant,
}

impl Shared {
    pub fn new(info: Info, protocol: EcuProtocol, mqtt_enabled: bool) -> Arc<Self> {
        let (bus, _) = broadcast::channel(BUS_CAPACITY);
        Arc::new(Self {
            info,
            counters: Counters::default(),
            bus,
            snapshot: RwLock::new(Snapshot::new(protocol, mqtt_enabled)),
            started: Instant::now(),
        })
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Snapshot> {
        self.snapshot.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Snapshot> {
        self.snapshot.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_link(&self, status: LinkStatus, detail: impl Into<String>) {
        let detail = detail.into();
        let mut s = self.write();
        if s.link != status {
            s.link = status;
            s.link_since = Instant::now();
        }
        if !detail.is_empty() {
            s.link_detail = detail;
        }
    }

    pub fn set_mqtt(&self, status: LinkStatus) {
        self.write().mqtt = status;
    }

    /// Healthy = ECU link up and a sample within the channel freshness window.
    pub fn health(&self) -> Health {
        let now = Instant::now();
        let s = self.read();
        let age = s.sample_age_s(now);
        let fresh = age.is_some_and(|a| a * 1000.0 <= CHANNEL_MAX_AGE_MS as f64);
        let status = match (s.link.is_up(), fresh) {
            (true, true) => "ok",
            (true, false) => "no_data",
            (false, _) => "ecu_down",
        };
        Health {
            status,
            link: s.link,
            link_detail: s.link_detail.clone(),
            protocol: s.protocol.name(),
            firmware: s.firmware.clone(),
            mqtt: s.mqtt,
            sample_age_s: age,
            sample_rate_hz: s.sample_rate,
            poll_latency_ms: s.poll_latency_ms,
            uptime_s: now.duration_since(self.started).as_secs(),
            version: self.info.version,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub link: LinkStatus,
    pub link_detail: String,
    pub protocol: &'static str,
    pub firmware: Option<String>,
    pub mqtt: LinkStatus,
    pub sample_age_s: Option<f64>,
    pub sample_rate_hz: f64,
    pub poll_latency_ms: Option<f64>,
    pub uptime_s: u64,
    pub version: &'static str,
}

impl Health {
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_tracks_link_and_freshness() {
        let shared = Shared::new(Info::default(), EcuProtocol::Speeduino, false);
        assert_eq!(shared.health().status, "ecu_down");
        shared.set_link(LinkStatus::Connected, "/dev/ttyUSB0 @ 115200 baud");
        assert_eq!(shared.health().status, "no_data");
        shared.write().last_sample_at = Some(Instant::now());
        let h = shared.health();
        assert!(h.is_ok());
        assert_eq!(h.link_detail, "/dev/ttyUSB0 @ 115200 baud");
        assert_eq!(h.mqtt, LinkStatus::Disabled);
        shared.set_link(LinkStatus::Disconnected, "");
        assert_eq!(shared.health().status, "ecu_down");
    }

    #[test]
    fn fresh_channels_excludes_stale_ones() {
        let shared = Shared::new(Info::default(), EcuProtocol::HaltechCanV2, false);
        let now = Instant::now();
        {
            let mut s = shared.write();
            s.channels.insert(
                "rpm".into(),
                ChannelState {
                    value: 1.0,
                    captured_ms: 0,
                    source: "x",
                    at: now,
                },
            );
            s.channels.insert(
                "coolantC".into(),
                ChannelState {
                    value: 1.0,
                    captured_ms: 0,
                    source: "x",
                    at: now - std::time::Duration::from_secs(5),
                },
            );
        }
        let s = shared.read();
        let fresh: Vec<&String> = s.fresh_channels(now).map(|(k, _)| k).collect();
        assert_eq!(fresh, ["rpm"]);
    }
}
