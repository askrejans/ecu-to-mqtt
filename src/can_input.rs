//! Shared handling for received CAN frames, used by both the native SocketCAN
//! reader and the TCP gateway transport. Decoding, freshness and publishing
//! behave identically whichever way the frame arrived.

use crate::{
    config::AppConfig,
    ecu_protocol::{CanInputFrame, decode_can},
    mqtt_handler::MqttMessage,
    telemetry_frame,
    tui::TuiState,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, mpsc};

/// Frames captured more than this long ago (or dated this far in the future)
/// are dropped instead of being published as live telemetry.
const MAX_FRAME_AGE_MS: u64 = 2000;

/// Identifier the configured profile broadcasts from, honouring an override.
pub fn base_id(config: &AppConfig) -> u32 {
    config
        .can_base_id
        .unwrap_or_else(|| config.ecu_protocol.default_can_base())
}

pub fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_millis() as u64)
}

/// Decode one frame and forward it to the terminal dashboard and MQTT.
/// Stale frames, other profiles' identifiers and disabled broadcast groups
/// produce no telemetry, so an absent group can never refresh another
/// group's channels.
pub async fn accept(
    frame: CanInputFrame,
    base: u32,
    config: &AppConfig,
    sender: Option<&mpsc::Sender<MqttMessage>>,
    tui_state: &Arc<RwLock<TuiState>>,
) -> anyhow::Result<()> {
    if let (Some(captured), Some(now)) = (frame.timestamp_ms, now_ms())
        && now.abs_diff(captured) > MAX_FRAME_AGE_MS
    {
        return Ok(());
    }
    match decode_can(config.ecu_protocol, base, &frame) {
        Ok(channels) if !channels.is_empty() => {
            {
                let mut state = tui_state.write().await;
                state.update_channels(&channels);
                if sender.is_some() {
                    state.messages_published = state.messages_published.saturating_add(1);
                }
            }
            telemetry_frame::publish_at(
                channels,
                config.ecu_protocol.source(),
                true,
                config,
                sender,
                frame.timestamp_ms,
            )
            .await?;
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "CAN frame rejected"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu_protocol::EcuProtocol;

    fn haltech() -> AppConfig {
        AppConfig {
            ecu_protocol: EcuProtocol::HaltechCanV2,
            connection_type: "can".into(),
            can_interface: Some("vcan0".into()),
            ..Default::default()
        }
    }

    fn rpm_frame(timestamp_ms: Option<u64>) -> CanInputFrame {
        CanInputFrame {
            id: 0x360,
            extended: false,
            data: vec![0x17, 0x70, 0x03, 0xf5, 0x02, 0xee, 0, 0],
            timestamp_ms,
        }
    }

    #[test]
    fn base_identifier_falls_back_to_the_profile_default() {
        assert_eq!(base_id(&haltech()), 0x360);
        assert_eq!(
            base_id(&AppConfig {
                can_base_id: Some(0x520),
                ..haltech()
            }),
            0x520
        );
    }

    #[tokio::test]
    async fn fresh_frame_reaches_both_the_dashboard_and_mqtt() {
        let (sender, mut receiver) = mpsc::channel(4);
        let state = Arc::new(RwLock::new(TuiState::default()));
        accept(
            rpm_frame(now_ms()),
            0x360,
            &haltech(),
            Some(&sender),
            &state,
        )
        .await
        .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&receiver.recv().await.unwrap().payload).unwrap();
        assert_eq!(payload["channels"]["rpm"], 6000.);
        assert_eq!(payload["partial"], true);
        assert_eq!(state.read().await.channels["rpm"].value, 6000.);
    }

    #[tokio::test]
    async fn frames_buffered_for_too_long_are_discarded() {
        let (sender, mut receiver) = mpsc::channel(4);
        let state = Arc::new(RwLock::new(TuiState::default()));
        let stale = now_ms().map(|now| now - MAX_FRAME_AGE_MS - 500);
        accept(rpm_frame(stale), 0x360, &haltech(), Some(&sender), &state)
            .await
            .unwrap();
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.read().await.frames_decoded, 0);
    }

    #[tokio::test]
    async fn frames_from_other_identifiers_publish_nothing() {
        let (sender, mut receiver) = mpsc::channel(4);
        let state = Arc::new(RwLock::new(TuiState::default()));
        let other = CanInputFrame {
            id: 0x7ff,
            ..rpm_frame(now_ms())
        };
        accept(other, 0x360, &haltech(), Some(&sender), &state)
            .await
            .unwrap();
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.read().await.frames_decoded, 0);
    }
}
