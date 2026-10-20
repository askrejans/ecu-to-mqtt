//! Decoded telemetry samples and the canonical `telemetry` envelope.
//!
//! Envelope (schema 1, consumed by the G86 app's `EcuTelemetryDecoder`):
//!
//! ```json
//! {"schema":1,"source":"speeduino-primary-a","partial":false,"bootId":"…",
//!  "timestampMs":1750000000000,"sequence":42,"channels":{"rpm":6000.0,…}}
//! ```
//!
//! `timestampMs` is the capture time (CAN kernel timestamp or poll completion),
//! `bootId` changes per process start and `sequence` increases monotonically.

use crate::protocol::Channels;
use crate::protocol::speeduino::SpeeduinoData;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One decoded packet or CAN frame.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Canonical source identifier, e.g. `speeduino-primary-a`, `haltech-can-v2`.
    pub source: &'static str,
    /// True when the sample updates only the listed channels (CAN groups).
    pub partial: bool,
    /// Capture time, Unix milliseconds.
    pub captured_ms: u64,
    /// Canonical, range-checked channels.
    pub channels: Channels,
    /// Full Speeduino packet (legacy scalar topics, dashboard detail).
    pub speeduino: Option<Arc<SpeeduinoData>>,
}

impl Sample {
    pub fn new(source: &'static str, partial: bool, captured_ms: u64, channels: Channels) -> Self {
        Self {
            source,
            partial,
            captured_ms,
            channels,
            speeduino: None,
        }
    }
}

/// A sample stamped by the hub with its envelope identity.
#[derive(Debug, Clone)]
pub struct Stamped {
    pub sequence: u64,
    pub sample: Sample,
    /// Pre-serialised canonical envelope (shared by MQTT, archive and SSE).
    pub envelope: Arc<str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    schema: u8,
    source: &'a str,
    partial: bool,
    boot_id: &'a str,
    timestamp_ms: u64,
    sequence: u64,
    channels: &'a Channels,
}

/// Serialise the canonical envelope.
pub fn envelope(sample: &Sample, boot_id: &str, sequence: u64) -> String {
    serde_json::to_string(&Envelope {
        schema: 1,
        source: sample.source,
        partial: sample.partial,
        boot_id,
        timestamp_ms: sample.captured_ms,
        sequence,
        channels: &sample.channels,
    })
    .unwrap_or_default()
}

/// Process-unique boot identifier.
pub fn new_boot_id() -> String {
    format!("{}-{}", std::process::id(), now_ms())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_matches_the_g86_contract() {
        let sample = Sample::new(
            "haltech-can-v2",
            true,
            1_750_000_000_000,
            Channels::from([("rpm".into(), 6000.0)]),
        );
        let v: serde_json::Value = serde_json::from_str(&envelope(&sample, "boot-1", 42)).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["source"], "haltech-can-v2");
        assert_eq!(v["partial"], true);
        assert_eq!(v["bootId"], "boot-1");
        assert_eq!(v["timestampMs"], 1_750_000_000_000u64);
        assert_eq!(v["sequence"], 42);
        assert_eq!(v["channels"]["rpm"], 6000.0);
        assert_eq!(v.as_object().unwrap().len(), 7);
    }
}
