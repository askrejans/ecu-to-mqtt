//! ECU input sources. Each source runs in its own task, reconnects with
//! backoff, decodes in place and sends [`SourceEvent`]s to the hub.

mod can_gateway;
pub mod can_input;
mod can_socket;
pub mod link;
pub mod poller;
mod simulator;

use crate::config::{AppConfig, ConnectionKind};
use crate::hub::SourceEvent;
use crate::shared::LinkStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub type Tx = mpsc::Sender<SourceEvent>;

/// Exponential reconnect backoff between `min` and `max`.
pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min_ms: u64, max_ms: u64) -> Self {
        let min = Duration::from_millis(min_ms.max(1));
        Self {
            min,
            max: Duration::from_millis(max_ms).max(min),
            next: min,
        }
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(self.max);
        d
    }
}

/// Allows one log line per period, so faults at poll rate do not flood logs.
pub struct RateLimit {
    period: Duration,
    last: Option<Instant>,
}

impl RateLimit {
    pub fn new(period: Duration) -> Self {
        Self { period, last: None }
    }

    pub fn ready(&mut self) -> bool {
        if self.last.is_none_or(|t| t.elapsed() >= self.period) {
            self.last = Some(Instant::now());
            true
        } else {
            false
        }
    }
}

pub fn send_link(tx: &Tx, status: LinkStatus, detail: impl Into<String>) {
    let _ = tx.try_send(SourceEvent::Link(status, detail.into()));
}

/// Sleep for the backoff delay unless cancelled. Returns `false` when cancelled.
pub async fn wait(backoff: &mut Backoff, cancel: &CancellationToken) -> bool {
    let delay = backoff.next_delay();
    tracing::debug!("Reconnecting in {delay:?}");
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = cancel.cancelled() => false,
    }
}

pub fn spawn(config: &Arc<AppConfig>, tx: Tx, cancel: CancellationToken) {
    let config = config.clone();
    match config.connection_kind() {
        ConnectionKind::Simulator => {
            tokio::spawn(async move { simulator::run(config, tx, cancel).await });
        }
        ConnectionKind::Can => {
            tokio::spawn(async move {
                if let Err(e) = can_socket::run(config, tx, cancel).await {
                    tracing::error!("{e:#}");
                }
            });
        }
        ConnectionKind::Tcp if config.ecu_protocol.is_can() => {
            tokio::spawn(async move { can_gateway::run(config, tx, cancel).await });
        }
        ConnectionKind::Serial | ConnectionKind::Tcp => {
            tokio::spawn(async move { poller::run(config, tx, cancel).await });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let mut b = Backoff::new(1000, 8000);
        let seq: Vec<u64> = (0..5).map(|_| b.next_delay().as_millis() as u64).collect();
        assert_eq!(seq, [1000, 2000, 4000, 8000, 8000]);
        b.reset();
        assert_eq!(b.next_delay().as_millis(), 1000);
    }

    #[test]
    fn rate_limit_allows_one_per_period() {
        let mut r = RateLimit::new(Duration::from_secs(60));
        assert!(r.ready());
        assert!(!r.ready());
    }
}
