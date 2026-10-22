//! The hub: the single consumer of decoded samples.
//!
//! Sources decode and send [`SourceEvent`]s; the hub stamps the canonical
//! envelope (`bootId`, monotonic `sequence`), updates the snapshot, queues the
//! archive and broadcasts to outputs. It never awaits an output.

use crate::archive;
use crate::protocol::EcuProtocol;
use crate::sample::{self, Sample, Stamped};
use crate::shared::{ChannelState, Counters, LinkStatus, Shared};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Debug)]
pub enum SourceEvent {
    Sample(Box<Sample>),
    Link(LinkStatus, String),
    /// ECU signature and the profile resolved from it.
    Identified(String, EcuProtocol),
    /// Request → response time of a serial poll, milliseconds.
    Latency(f64),
    /// A reply or frame that could not be decoded.
    DecodeError,
    /// A poll that received no reply.
    Timeout,
}

pub async fn run(
    mut rx: mpsc::Receiver<SourceEvent>,
    shared: Arc<Shared>,
    archive: Option<mpsc::Sender<Arc<str>>>,
    cancel: CancellationToken,
) {
    let boot_id = shared.boot_id.clone();
    let mut sequence = 0u64;
    let mut this_second = 0u64;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = rx.recv() => match event {
                Some(SourceEvent::Sample(sample)) => {
                    let stamped = stamp(*sample, &boot_id, &mut sequence);
                    this_second += 1;
                    record(&shared, &stamped);
                    if let Some(tx) = &archive {
                        archive::record(tx, &shared, stamped.envelope.clone());
                    }
                    let _ = shared.bus.send(Arc::new(stamped));
                }
                Some(SourceEvent::Link(status, detail)) => {
                    if status == LinkStatus::Disconnected && shared.read().link == LinkStatus::Connected {
                        Counters::inc(&shared.counters.reconnects);
                    }
                    shared.set_link(status, detail);
                }
                Some(SourceEvent::DecodeError) => Counters::inc(&shared.counters.decode_errors),
                Some(SourceEvent::Timeout) => Counters::inc(&shared.counters.timeouts),
                Some(SourceEvent::Identified(signature, protocol)) => {
                    info!("ECU identified: {signature} ({})", protocol.label());
                    let mut s = shared.write();
                    s.firmware = Some(signature);
                    s.protocol = protocol;
                }
                Some(SourceEvent::Latency(ms)) => shared.write().poll_latency_ms = Some(ms),
                None => break,
            },
            _ = tick.tick() => {
                shared.write().sample_rate = this_second as f64;
                this_second = 0;
            }
        }
    }
    info!("Hub stopped");
}

fn stamp(sample: Sample, boot_id: &str, sequence: &mut u64) -> Stamped {
    let seq = *sequence;
    *sequence += 1;
    let envelope: Arc<str> = Arc::from(sample::envelope(&sample, boot_id, seq));
    Stamped {
        sequence: seq,
        sample,
        envelope,
    }
}

fn record(shared: &Shared, stamped: &Stamped) {
    Counters::inc(&shared.counters.samples);
    let now = Instant::now();
    let sample = &stamped.sample;
    let mut s = shared.write();
    s.last_sample_at = Some(now);
    s.last_sequence = stamped.sequence;
    if !sample.partial {
        // A complete packet replaces the channel set: channels it no longer
        // carries (sensor disconnected) must not linger as live values.
        s.channels.clear();
    }
    for (name, value) in &sample.channels {
        s.channels.insert(
            name.clone(),
            ChannelState {
                value: *value,
                captured_ms: sample.captured_ms,
                source: sample.source,
                at: now,
            },
        );
    }
    if let Some(detail) = &sample.speeduino {
        s.speeduino = Some(detail.clone());
    }
    if s.link != LinkStatus::Connected {
        s.link = LinkStatus::Connected;
        s.link_since = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Channels;
    use crate::shared::Info;

    #[tokio::test]
    async fn samples_are_stamped_recorded_and_broadcast() {
        let shared = Shared::new(Info::default(), EcuProtocol::HaltechCanV2, false);
        let mut bus = shared.bus.subscribe();
        let (tx, rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(rx, shared.clone(), None, cancel.clone()));

        for (rpm, partial) in [(1000.0, true), (2000.0, true)] {
            let s = Sample::new(
                "haltech-can-v2",
                partial,
                1,
                Channels::from([("rpm".into(), rpm)]),
            );
            tx.send(SourceEvent::Sample(Box::new(s))).await.unwrap();
        }
        tx.send(SourceEvent::Sample(Box::new(Sample::new(
            "haltech-can-v2",
            true,
            2,
            Channels::from([("coolantC".into(), 90.0)]),
        ))))
        .await
        .unwrap();

        let mut seqs = Vec::new();
        for _ in 0..3 {
            let s = tokio::time::timeout(Duration::from_secs(1), bus.recv())
                .await
                .unwrap()
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(&s.envelope).unwrap();
            seqs.push(v["sequence"].as_u64().unwrap());
            assert!(!v["bootId"].as_str().unwrap().is_empty());
        }
        assert_eq!(seqs, [0, 1, 2]);
        {
            let s = shared.read();
            assert_eq!(s.channels["rpm"].value, 2000.0);
            assert!(s.channels.contains_key("coolantC"), "partial samples merge");
            assert_eq!(s.link, LinkStatus::Connected);
        }
        // A complete (non-partial) packet replaces the channel set.
        tx.send(SourceEvent::Sample(Box::new(Sample::new(
            "speeduino-primary-a",
            false,
            3,
            Channels::from([("rpm".into(), 800.0)]),
        ))))
        .await
        .unwrap();
        bus.recv().await.unwrap();
        assert!(!shared.read().channels.contains_key("coolantC"));
        tx.send(SourceEvent::Identified(
            "speeduino 202501".into(),
            EcuProtocol::Speeduino,
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(shared.read().firmware.as_deref(), Some("speeduino 202501"));
        cancel.cancel();
        task.await.unwrap();
    }
}
