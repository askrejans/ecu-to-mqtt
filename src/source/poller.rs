//! Request/response poller for serial ECUs (Speeduino, MegaSquirt) over a
//! serial port or a TCP serial bridge.
//!
//! Only documented read commands are ever sent:
//! * `Q` — signature (identification, once per connection)
//! * `A` — Speeduino realtime packet
//! * `a 00 06` — MegaSquirt 112-byte compatibility realtime block
//!
//! A reply completes as soon as the expected number of bytes has arrived, so
//! the poll rate is limited by the ECU (≈ 14 ms for a 130-byte Speeduino
//! packet at 115200 baud), not by a fixed sleep.

use super::link::{self, Link};
use super::{Backoff, RateLimit, Tx, send_link};
use crate::config::{AppConfig, ConnectionKind};
use crate::hub::SourceEvent;
use crate::protocol::{self, EcuProtocol, speeduino};
use crate::sample::{Sample, now_ms};
use crate::shared::LinkStatus;
use anyhow::{Result, bail};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Gap after the last byte that ends a reply of unknown length. Generous
/// because USB-UART bridges deliver data in bursts (FTDI latency timer 16 ms).
const IDLE_GAP: Duration = Duration::from_millis(25);
/// Time an Arduino-based board needs after a DTR reset before it answers.
const BOOT_WAIT: Duration = Duration::from_millis(2000);
const MAX_REPLY: usize = 512;

#[derive(Debug)]
pub enum PollError {
    Timeout,
    Io(std::io::Error),
}

/// Read one reply: until `expected` bytes, an idle gap after data, or the deadline.
pub async fn read_reply(
    link: &mut Link,
    expected: Option<usize>,
    deadline: Duration,
    idle: Duration,
) -> std::result::Result<Vec<u8>, PollError> {
    let start = Instant::now();
    let mut buf = Vec::with_capacity(expected.unwrap_or(256).min(MAX_REPLY));
    let mut chunk = [0u8; 256];
    loop {
        if expected.is_some_and(|n| buf.len() >= n) || buf.len() >= MAX_REPLY {
            break;
        }
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        // A known length is read to completion (bridges split replies with
        // pauses); only an unknown length ends at an idle gap.
        let wait = if buf.is_empty() || expected.is_some() {
            remaining
        } else {
            idle.min(remaining)
        };
        match tokio::time::timeout(wait, link.read(&mut chunk)).await {
            Ok(Ok(0)) => return Err(PollError::Io(std::io::ErrorKind::UnexpectedEof.into())),
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Ok(Err(e)) => return Err(PollError::Io(e)),
            Err(_) => break, // idle gap or deadline
        }
    }
    if let Some(n) = expected {
        buf.truncate(n);
    }
    if buf.is_empty() {
        Err(PollError::Timeout)
    } else {
        Ok(buf)
    }
}

/// Send `Q` and return the printable signature, if any.
pub async fn identify(link: &mut Link, attempts: u32) -> Option<String> {
    for attempt in 0..attempts {
        link.clear_input();
        if link.write_all(b"Q").await.is_err() || link.flush().await.is_err() {
            return None;
        }
        if let Ok(reply) = read_reply(link, None, Duration::from_millis(400), IDLE_GAP).await {
            let text: String = reply
                .iter()
                .take_while(|b| **b != 0)
                .map(|&b| b as char)
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .collect();
            let text = text.trim().to_string();
            if text.len() >= 3 {
                return Some(text);
            }
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    None
}

struct Connection {
    link: Link,
    detail: String,
    protocol: EcuProtocol,
    signature: Option<String>,
}

/// Resolve the profile for a signature, honouring an explicit configuration.
fn resolve(configured: EcuProtocol, signature: Option<&str>) -> Option<EcuProtocol> {
    let detected = signature.and_then(EcuProtocol::from_signature);
    match configured {
        EcuProtocol::Auto => detected,
        explicit => {
            if let (Some(sig), Some(d)) = (signature, detected) {
                let family = |p: EcuProtocol| p.is_megasquirt_serial();
                if family(d) != family(explicit) {
                    warn!(
                        "ECU signature {sig:?} looks like {} but ecu_protocol = {}",
                        d.name(),
                        explicit.name()
                    );
                }
            }
            Some(explicit)
        }
    }
}

async fn open(config: &AppConfig, cancel: &CancellationToken) -> Result<Connection> {
    let configured = config.ecu_protocol;
    match config.connection_kind() {
        ConnectionKind::Tcp => {
            let host = config.tcp_host.as_deref().unwrap_or_default();
            let port = config.tcp_port.unwrap_or_default();
            let mut link = Link::open_tcp(host, port).await?;
            let signature = identify(&mut link, 3).await;
            let Some(protocol) = resolve(configured, signature.as_deref()) else {
                bail!("no recognised ECU signature from {host}:{port} (got {signature:?})");
            };
            Ok(Connection {
                link,
                detail: format!("TCP {host}:{port}"),
                protocol,
                signature,
            })
        }
        _ if config.port_name.eq_ignore_ascii_case("auto") => {
            // Only USB adapters are probed: each probe costs a boot wait, and
            // built-in UARTs (ttyS*, Pi GPIO serial) need an explicit port_name.
            let candidates: Vec<_> = link::candidates()
                .into_iter()
                .filter(|c| c.score >= link::USB_SCORE)
                .collect();
            if candidates.is_empty() {
                bail!("no USB serial ports found; set port_name for a built-in UART");
            }
            for c in candidates {
                if cancel.is_cancelled() {
                    bail!("cancelled");
                }
                let Ok(mut link) = Link::open_serial(&c.path, config.baud_rate) else {
                    continue;
                };
                tokio::time::sleep(BOOT_WAIT).await;
                let signature = identify(&mut link, 2).await;
                let detected = signature.as_deref().and_then(EcuProtocol::from_signature);
                if detected.is_some() {
                    let protocol = resolve(configured, signature.as_deref()).unwrap_or(configured);
                    info!(
                        "Detected ECU on {} ({}) — {}",
                        c.path,
                        c.description,
                        signature.as_deref().unwrap_or("")
                    );
                    return Ok(Connection {
                        link,
                        detail: format!("{} @ {} baud", c.path, config.baud_rate),
                        protocol,
                        signature,
                    });
                }
                debug!("{}: no ECU signature ({signature:?})", c.path);
            }
            bail!(
                "no ECU answered the signature request on any serial port at {} baud",
                config.baud_rate
            )
        }
        _ => {
            let mut link = Link::open_serial(&config.port_name, config.baud_rate)?;
            tokio::time::sleep(BOOT_WAIT).await;
            let signature = identify(&mut link, 3).await;
            let Some(protocol) = resolve(configured, signature.as_deref()) else {
                bail!(
                    "no recognised ECU signature on {} (got {signature:?})",
                    config.port_name
                );
            };
            Ok(Connection {
                link,
                detail: format!("{} @ {} baud", config.port_name, config.baud_rate),
                protocol,
                signature,
            })
        }
    }
}

/// Decode a reply into a sample.
pub fn decode(
    protocol: EcuProtocol,
    reply: &[u8],
    captured_ms: u64,
) -> crate::errors::Result<Sample> {
    if protocol.is_megasquirt_serial() {
        let channels = protocol::decode_compat112(reply)?;
        return Ok(Sample::new(protocol.source(), false, captured_ms, channels));
    }
    let data = speeduino::parse(reply)?;
    let mut sample = Sample::new(
        protocol.source(),
        false,
        captured_ms,
        speeduino::canonical(&data),
    );
    sample.speeduino = Some(Arc::new(data));
    Ok(sample)
}

pub async fn run(config: Arc<AppConfig>, tx: Tx, cancel: CancellationToken) {
    let mut backoff = Backoff::new(config.initial_retry_delay_ms, config.max_retry_delay_ms);
    let mut failures_log = RateLimit::new(Duration::from_secs(10));
    while !cancel.is_cancelled() {
        send_link(&tx, LinkStatus::Connecting, config.connection_display());
        let conn = tokio::select! {
            _ = cancel.cancelled() => return,
            c = open(&config, &cancel) => c,
        };
        match conn {
            Ok(conn) => {
                backoff.reset();
                if let Some(sig) = &conn.signature {
                    let _ = tx
                        .send(SourceEvent::Identified(sig.clone(), conn.protocol))
                        .await;
                }
                info!("ECU link open: {} ({})", conn.detail, conn.protocol.label());
                send_link(&tx, LinkStatus::Connected, conn.detail.clone());
                if poll_loop(conn, &config, &tx, &cancel, &mut failures_log).await {
                    return;
                }
            }
            Err(e) if cancel.is_cancelled() => {
                debug!("{e:#}");
                return;
            }
            Err(e) => {
                if failures_log.ready() {
                    warn!("ECU unavailable: {e:#}");
                }
            }
        }
        send_link(&tx, LinkStatus::Disconnected, "");
        if !super::wait(&mut backoff, &cancel).await {
            return;
        }
    }
}

/// Poll until the link fails. Returns true when shutting down.
async fn poll_loop(
    mut conn: Connection,
    config: &AppConfig,
    tx: &Tx,
    cancel: &CancellationToken,
    log: &mut RateLimit,
) -> bool {
    let protocol = conn.protocol;
    let mut expected = if protocol.is_megasquirt_serial() {
        112
    } else {
        config.expected_data_length
    };
    let mut learned = protocol.is_megasquirt_serial();
    let timeout = Duration::from_millis(config.read_timeout_ms);
    let mut interval = tokio::time::interval(Duration::from_millis(config.refresh_rate_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut consecutive = 0u32;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return true,
            _ = interval.tick() => {}
        }
        conn.link.clear_input();
        let started = Instant::now();
        if let Err(e) = async {
            conn.link.write_all(protocol.command()).await?;
            conn.link.flush().await
        }
        .await
        {
            warn!("ECU write failed: {e}");
            return false;
        }
        // Until the length is confirmed, read to an idle gap so a 138-byte
        // firmware is recognised instead of being truncated to 130.
        let want = if learned { Some(expected) } else { None };
        match read_reply(&mut conn.link, want, timeout, IDLE_GAP).await {
            Ok(reply) => {
                let captured = now_ms();
                let _ = tx.try_send(SourceEvent::Latency(
                    started.elapsed().as_secs_f64() * 1000.0,
                ));
                if !learned && reply.len() >= speeduino::PACKET_LEN {
                    if reply.len() != expected {
                        info!(
                            "Speeduino sends {}-byte packets (configured {expected})",
                            reply.len()
                        );
                    }
                    expected = reply.len();
                    learned = true;
                }
                match decode(protocol, &reply, captured) {
                    Ok(sample) => {
                        consecutive = 0;
                        if tx
                            .send(SourceEvent::Sample(Box::new(sample)))
                            .await
                            .is_err()
                        {
                            return true;
                        }
                    }
                    Err(e) => {
                        consecutive += 1;
                        let _ = tx.try_send(SourceEvent::DecodeError);
                        if log.ready() {
                            warn!("ECU reply rejected ({} bytes): {e}", reply.len());
                        }
                    }
                }
            }
            Err(PollError::Timeout) => {
                consecutive += 1;
                let _ = tx.try_send(SourceEvent::Timeout);
                if log.ready() {
                    warn!("No reply from ECU within {} ms", config.read_timeout_ms);
                }
            }
            Err(PollError::Io(e)) => {
                warn!("ECU link error: {e}");
                return false;
            }
        }
        if consecutive >= config.max_retry_count {
            warn!("{consecutive} consecutive failed polls; reconnecting");
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    async fn tcp_pair() -> (Link, tokio::net::TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client =
            tokio::spawn(async move { Link::open_tcp("127.0.0.1", addr.port()).await.unwrap() });
        let (server, _) = listener.accept().await.unwrap();
        (client.await.unwrap(), server)
    }

    #[tokio::test]
    async fn reply_completes_at_expected_length_not_after_a_fixed_wait() {
        let (mut link, mut ecu) = tcp_pair().await;
        ecu.write_all(&[7u8; 130]).await.unwrap();
        let started = Instant::now();
        let reply = read_reply(&mut link, Some(130), Duration::from_secs(2), IDLE_GAP)
            .await
            .unwrap();
        assert_eq!(reply.len(), 130);
        assert!(
            started.elapsed() < Duration::from_millis(20),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn fragmented_reply_is_reassembled_and_unknown_length_ends_on_idle() {
        let (mut link, mut ecu) = tcp_pair().await;
        tokio::spawn(async move {
            ecu.write_all(&[1u8; 60]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            ecu.write_all(&[2u8; 78]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let reply = read_reply(&mut link, None, Duration::from_secs(2), IDLE_GAP)
            .await
            .unwrap();
        assert_eq!(reply.len(), 138);
    }

    #[tokio::test]
    async fn known_length_survives_a_pause_mid_reply() {
        let (mut link, mut ecu) = tcp_pair().await;
        tokio::spawn(async move {
            ecu.write_all(&[1u8; 37]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(180)).await;
            ecu.write_all(&[2u8; 75]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let reply = read_reply(&mut link, Some(112), Duration::from_secs(2), IDLE_GAP)
            .await
            .unwrap();
        assert_eq!(reply.len(), 112);
    }

    #[tokio::test]
    async fn silence_is_a_timeout() {
        let (mut link, _ecu) = tcp_pair().await;
        assert!(matches!(
            read_reply(&mut link, Some(130), Duration::from_millis(50), IDLE_GAP).await,
            Err(PollError::Timeout)
        ));
    }

    #[test]
    fn explicit_profile_wins_auto_follows_signature() {
        assert_eq!(
            resolve(EcuProtocol::Auto, Some("speeduino 202501")),
            Some(EcuProtocol::Speeduino)
        );
        assert_eq!(
            resolve(EcuProtocol::Auto, Some("MS3 Format 0566")),
            Some(EcuProtocol::Ms3)
        );
        assert_eq!(resolve(EcuProtocol::Auto, None), None);
        assert_eq!(resolve(EcuProtocol::Ms2, None), Some(EcuProtocol::Ms2));
        assert_eq!(
            resolve(EcuProtocol::Speeduino, Some("MS3 x")),
            Some(EcuProtocol::Speeduino)
        );
    }

    /// End-to-end over TCP: signature, then `A` polls answered with the real
    /// 2025.01 packet captured from a CORE4 board.
    #[tokio::test]
    async fn speeduino_over_tcp_identifies_and_streams_samples() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(AppConfig {
            ecu_protocol: EcuProtocol::Auto,
            connection_type: "tcp".into(),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(port),
            refresh_rate_ms: 10,
            ..AppConfig::default()
        });
        let (tx, mut rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(config, tx, cancel.clone()));
        let (mut ecu, _) = listener.accept().await.unwrap();
        let packet = include_bytes!("../../tests/fixtures/speeduino-202501-130.bin");
        tokio::spawn(async move {
            let mut cmd = [0u8; 1];
            while ecu.read_exact(&mut cmd).await.is_ok() {
                match cmd[0] {
                    b'Q' => ecu.write_all(b"speeduino 202501").await.unwrap(),
                    b'A' => ecu.write_all(packet).await.unwrap(),
                    other => {
                        panic!("unexpected command byte {other:#x}: the bridge must only read")
                    }
                }
            }
        });
        let mut identified = false;
        let mut samples = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while samples < 5 && Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                SourceEvent::Identified(sig, p) => {
                    assert_eq!(
                        (sig.as_str(), p),
                        ("speeduino 202501", EcuProtocol::Speeduino)
                    );
                    identified = true;
                }
                SourceEvent::Sample(s) => {
                    assert_eq!(s.source, "speeduino-primary-a");
                    assert!(!s.partial);
                    assert_eq!(s.channels["coolantC"], 82.0);
                    assert!(s.speeduino.is_some());
                    samples += 1;
                }
                _ => {}
            }
        }
        assert!(identified);
        assert_eq!(samples, 5);
        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn megasquirt_compat_reply_is_decoded() {
        let mut reply = [0u8; 112];
        reply[6..8].copy_from_slice(&6000u16.to_be_bytes());
        reply[22..24].copy_from_slice(&1940u16.to_be_bytes());
        reply[26..28].copy_from_slice(&138u16.to_be_bytes());
        let s = decode(EcuProtocol::Ms3, &reply, 1).unwrap();
        assert_eq!(s.source, "ms3-compat112");
        assert_eq!(s.channels["rpm"], 6000.0);
        assert_eq!(s.channels["coolantC"], 90.0);
        assert_eq!(s.channels["batteryV"], 13.8);
        assert!(decode(EcuProtocol::Speeduino, &[0u8; 75], 1).is_err());
    }
}
