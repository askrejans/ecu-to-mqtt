//! CAN frames from a USB/CAN gateway over TCP JSON-lines
//! (`scripts/can_gateway.py`). Each line is `{"id":1512,"data":[…8 bytes…]}`.
//! Receive only: nothing is ever sent to the gateway or the bus.

use super::{Backoff, RateLimit, Tx, can_input, send_link};
use crate::config::AppConfig;
use crate::hub::SourceEvent;
use crate::protocol::CanInputFrame;
use crate::shared::LinkStatus;
use std::sync::Arc;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpStream,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

/// Longest accepted JSON line; a peer that never sends a newline is dropped.
const MAX_LINE: usize = 4096;

pub async fn run(config: Arc<AppConfig>, tx: Tx, cancel: CancellationToken) {
    let host = config.tcp_host.clone().unwrap_or_default();
    let port = config.tcp_port.unwrap_or_default();
    let base = can_input::base_id(&config);
    let mut backoff = Backoff::new(config.initial_retry_delay_ms, config.max_retry_delay_ms);
    let mut reject_log = RateLimit::new(Duration::from_secs(10));
    loop {
        send_link(&tx, LinkStatus::Connecting, config.connection_display());
        let connected = tokio::select! {
            _ = cancel.cancelled() => return,
            r = timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), port))) => r,
        };
        if let Ok(Ok(stream)) = connected {
            backoff.reset();
            send_link(&tx, LinkStatus::Connected, config.connection_display());
            let mut reader = BufReader::new(stream);
            let mut line = Vec::with_capacity(256);
            loop {
                // fill_buf/consume bounds memory even if a peer never sends a newline.
                let bytes = tokio::select! {
                    _ = cancel.cancelled() => return,
                    r = timeout(Duration::from_secs(5), reader.fill_buf()) => r,
                };
                let Ok(Ok(bytes)) = bytes else { break };
                if bytes.is_empty() {
                    break;
                }
                let count = bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(bytes.len(), |i| i + 1);
                if line.len() + count > MAX_LINE {
                    tracing::warn!("Oversized CAN gateway line; reconnecting");
                    break;
                }
                line.extend_from_slice(&bytes[..count]);
                let complete = line.last() == Some(&b'\n');
                reader.consume(count);
                if !complete {
                    continue;
                }
                let event = match serde_json::from_slice::<CanInputFrame>(&line) {
                    Ok(frame) => match can_input::to_sample(&frame, base, &config) {
                        Ok(Some(sample)) => Some(SourceEvent::Sample(Box::new(sample))),
                        Ok(None) => None,
                        Err(e) => {
                            if reject_log.ready() {
                                tracing::warn!("CAN frame rejected: {e}");
                            }
                            Some(SourceEvent::DecodeError)
                        }
                    },
                    Err(e) => {
                        if reject_log.ready() {
                            tracing::warn!("Invalid CAN gateway JSON: {e}");
                        }
                        Some(SourceEvent::DecodeError)
                    }
                };
                line.clear();
                if let Some(event) = event
                    && tx.send(event).await.is_err()
                {
                    return;
                }
            }
        }
        send_link(&tx, LinkStatus::Disconnected, "");
        if !super::wait(&mut backoff, &cancel).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EcuProtocol;
    use crate::sample::now_ms;
    use tokio::{io::AsyncWriteExt, net::TcpListener, sync::mpsc};

    #[tokio::test]
    async fn fragmented_gateway_frames_become_samples_and_stale_ones_are_dropped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(AppConfig {
            connection_type: "tcp".into(),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(port),
            ecu_protocol: EcuProtocol::HaltechCanV2,
            ..Default::default()
        });
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(config, tx, cancel.clone()));
        let (mut stream, _) = listener.accept().await.unwrap();
        let now = now_ms();
        stream.write_all(b"{bad json}\n").await.unwrap();
        let stale =
            serde_json::json!({"id":864,"timestampMs":now-5000,"data":[23,112,3,245,2,238,0,0]})
                .to_string()
                + "\n";
        stream.write_all(stale.as_bytes()).await.unwrap();
        let packet = serde_json::json!({"id":864,"timestampMs":now,"data":[23,112,3,245,2,238,0,0]})
            .to_string() + "\n";
        // A split JSON line is still one telemetry packet.
        stream.write_all(&packet.as_bytes()[..11]).await.unwrap();
        stream.write_all(&packet.as_bytes()[11..]).await.unwrap();
        let mut samples = Vec::new();
        let mut decode_errors = 0;
        while samples.is_empty() {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                SourceEvent::Sample(s) => samples.push(s),
                SourceEvent::DecodeError => decode_errors += 1,
                _ => {}
            }
        }
        assert_eq!(
            decode_errors, 1,
            "bad JSON counted, stale frame silently dropped"
        );
        assert_eq!(samples[0].channels["rpm"], 6000.0);
        assert_eq!(samples[0].captured_ms, now);
        assert!(samples[0].partial);
        assert!(!samples[0].channels.contains_key("coolantC"));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}
