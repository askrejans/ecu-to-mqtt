use crate::config::AppConfig;
use crate::ecu_protocol::Channels;
use crate::errors::{ParseError, Result};
use crate::mqtt_handler::{MqttMessage, build_topic_path};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub async fn publish(
    channels: Channels,
    source: &str,
    partial: bool,
    config: &AppConfig,
    sender: Option<&mpsc::Sender<MqttMessage>>,
) -> Result<()> {
    publish_at(channels, source, partial, config, sender, None).await
}

pub async fn publish_at(
    channels: Channels,
    source: &str,
    partial: bool,
    config: &AppConfig,
    sender: Option<&mpsc::Sender<MqttMessage>>,
    captured_ms: Option<u64>,
) -> Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static BOOT_ID: OnceLock<String> = OnceLock::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ParseError::InvalidData {
            offset: 0,
            message: error.to_string(),
        })?;
    let boot = BOOT_ID.get_or_init(|| format!("{}-{}", std::process::id(), now.as_nanos()));
    let payload = serde_json::json!({
        "schema":1, "source":source, "partial":partial, "bootId":boot,
        "timestampMs":captured_ms.unwrap_or(now.as_millis() as u64), "sequence":SEQUENCE.fetch_add(1, Ordering::Relaxed),
        "channels":channels,
    })
    .to_string();
    tracing::debug!(
        source,
        channel_count = channels.len(),
        "Decoded ECU telemetry"
    );
    if let Some(sender) = sender {
        // Live telemetry must never block acquisition or graceful shutdown
        // behind a disconnected broker. Full queues drop the new sample;
        // capture timestamps let consumers discard older queued samples.
        match sender.try_send(MqttMessage::new(
            build_topic_path(&config.mqtt_base_topic, "telemetry"),
            payload,
            config.mqtt_qos,
        )) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!("MQTT queue full; dropped live ECU sample")
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return Err(ParseError::InvalidData {
                    offset: 0,
                    message: "MQTT queue closed".to_owned(),
                }
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn full_live_queue_does_not_block_acquisition() {
        let (sender, mut receiver) = mpsc::channel(1);
        let config = AppConfig::default();
        for rpm in [6000., 7000.] {
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                publish(
                    Channels::from([("rpm".into(), rpm)]),
                    "test",
                    true,
                    &config,
                    Some(&sender),
                ),
            )
            .await
            .unwrap()
            .unwrap();
        }
        let payload: serde_json::Value =
            serde_json::from_str(&receiver.recv().await.unwrap().payload).unwrap();
        assert_eq!(payload["channels"]["rpm"], 6000.);
        assert!(receiver.try_recv().is_err());
        drop(receiver);
        assert!(
            publish(Channels::new(), "test", true, &config, Some(&sender))
                .await
                .is_err()
        );
    }
}
