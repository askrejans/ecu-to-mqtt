//! Shared handling for received CAN frames, used by both the native SocketCAN
//! reader and the TCP gateway transport.

use crate::config::AppConfig;
use crate::protocol::{CanInputFrame, decode_can};
use crate::sample::{Sample, now_ms};

/// Frames captured more than this long ago (or dated this far in the future)
/// are dropped instead of being published as live telemetry.
pub const MAX_FRAME_AGE_MS: u64 = 2000;

/// Identifier the configured profile broadcasts from, honouring an override.
pub fn base_id(config: &AppConfig) -> u32 {
    config
        .can_base_id
        .unwrap_or_else(|| config.ecu_protocol.default_can_base())
}

/// Decode one frame. Stale frames, other profiles' identifiers and disabled
/// broadcast groups yield `Ok(None)`, so an absent group can never refresh
/// another group's channels.
pub fn to_sample(
    frame: &CanInputFrame,
    base: u32,
    config: &AppConfig,
) -> crate::errors::Result<Option<Sample>> {
    let now = now_ms();
    if frame
        .timestamp_ms
        .is_some_and(|captured| now.abs_diff(captured) > MAX_FRAME_AGE_MS)
    {
        return Ok(None);
    }
    let channels = decode_can(config.ecu_protocol, base, frame)?;
    if channels.is_empty() {
        return Ok(None);
    }
    Ok(Some(Sample::new(
        config.ecu_protocol.source(),
        true,
        frame.timestamp_ms.unwrap_or(now),
        channels,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EcuProtocol;

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

    #[test]
    fn fresh_frame_becomes_a_partial_sample() {
        let now = now_ms();
        let s = to_sample(&rpm_frame(Some(now)), 0x360, &haltech())
            .unwrap()
            .unwrap();
        assert_eq!(s.channels["rpm"], 6000.0);
        assert!(s.partial);
        assert_eq!(s.captured_ms, now);
        assert_eq!(s.source, "haltech-can-v2");
    }

    #[test]
    fn stale_foreign_and_malformed_frames_produce_nothing() {
        let stale = now_ms() - MAX_FRAME_AGE_MS - 500;
        assert!(
            to_sample(&rpm_frame(Some(stale)), 0x360, &haltech())
                .unwrap()
                .is_none()
        );
        let other = CanInputFrame {
            id: 0x7ff,
            ..rpm_frame(None)
        };
        assert!(to_sample(&other, 0x360, &haltech()).unwrap().is_none());
        let short = CanInputFrame {
            data: vec![0; 7],
            ..rpm_frame(None)
        };
        assert!(to_sample(&short, 0x360, &haltech()).is_err());
    }
}
