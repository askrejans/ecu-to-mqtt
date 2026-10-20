//! Optional local evidence archive. MQTT outages do not determine retention.
use crate::shared::{Counters, Shared};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::mpsc};

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const QUEUE: usize = 4096;

/// Most packets written per `fdatasync`.
const MAX_BATCH: usize = 256;

/// Append lines and `fdatasync` once. The path is reopened for every batch so
/// rename-based rotation takes effect immediately.
async fn append_batch(path: &str, packets: &[Arc<str>]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    let mut lines = Vec::with_capacity(packets.iter().map(|p| p.len() + 1).sum());
    for packet in packets {
        lines.extend_from_slice(packet.as_bytes());
        lines.push(b'\n');
    }
    if file.metadata().await?.len() + lines.len() as u64 > MAX_BYTES {
        return Err(io::Error::other(
            "telemetry archive reached 512 MiB; rotate or export it",
        ));
    }
    file.write_all(&lines).await?;
    file.sync_data().await
}

/// Spawn the archive writer. Packets are written in order on a dedicated task,
/// so a slow disk or `fsync` never stalls acquisition. If the queue is full
/// the packet is counted as an archive error and dropped (logged, rate-limited).
pub fn spawn(path: String, shared: Arc<Shared>) -> mpsc::Sender<Arc<str>> {
    let (tx, mut rx) = mpsc::channel::<Arc<str>>(QUEUE);
    tokio::spawn(async move {
        let mut last_warning: Option<Instant> = None;
        let mut batch = Vec::with_capacity(MAX_BATCH);
        while let Some(packet) = rx.recv().await {
            batch.push(packet);
            // Everything already queued shares one write and one sync.
            while batch.len() < MAX_BATCH {
                match rx.try_recv() {
                    Ok(p) => batch.push(p),
                    Err(_) => break,
                }
            }
            if let Err(error) = append_batch(&path, &batch).await {
                Counters::add(&shared.counters.archive_errors, batch.len() as u64);
                warn_rate_limited(&mut last_warning, &error);
            }
            batch.clear();
        }
    });
    tx
}

/// Storage failure must not fill the service journal at sensor frequency.
fn warn_rate_limited(last: &mut Option<Instant>, error: &dyn std::fmt::Display) {
    if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
        *last = Some(Instant::now());
        tracing::error!(%error, "Local telemetry archive unavailable; live capture continues without archiving");
    }
}

/// Queue a packet; never blocks.
pub fn record(tx: &mpsc::Sender<Arc<str>>, shared: &Shared, packet: Arc<str>) {
    if tx.try_send(packet).is_err() {
        Counters::inc(&shared.counters.archive_errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[tokio::test]
    async fn archive_preserves_packets_across_rotation_and_enforces_cap() {
        let directory = std::env::temp_dir().join(format!(
            "g86-archive-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let path = directory.join("telemetry.ndjson");
        let name = path.to_str().unwrap();
        append_batch(
            name,
            &[
                Arc::from(r#"{"sequence":1}"#),
                Arc::from(r#"{"sequence":2}"#),
            ],
        )
        .await
        .unwrap();
        let saved = directory.join("telemetry.ndjson.1");
        tokio::fs::rename(&path, &saved).await.unwrap();
        append_batch(name, &[Arc::from(r#"{"sequence":3}"#)])
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&saved)
                .await
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "{\"sequence\":3}\n"
        );
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.set_len(MAX_BYTES).await.unwrap();
        assert!(append_batch(name, &[Arc::from("{}")]).await.is_err());
        drop(file);
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
