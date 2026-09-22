//! Native SocketCAN input. Reads the ECU broadcast straight from a CAN
//! interface (`can0`, `vcan0`, a USB adapter brought up with `ip link`), so no
//! gateway process sits between the bus and the bridge.
//!
//! Receive only: the socket is never written to, and error, remote and CAN FD
//! frames are ignored. Bitrate and interface state are configured outside this
//! program with `ip link set can0 up type can bitrate 500000`, because that is
//! a privileged operation and the correct bitrate comes from the ECU's setup.

use crate::{config::AppConfig, mqtt_handler::MqttMessage, tui::TuiState};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "linux")]
pub async fn run(
    config: Arc<AppConfig>,
    sender: Option<mpsc::Sender<MqttMessage>>,
    tui_state: Arc<RwLock<TuiState>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    use crate::{can_input, ecu_protocol::CanInputFrame};
    use socketcan::{CanFrame, EmbeddedFrame, Id, SocketOptions, tokio::CanSocket};
    use std::time::{Duration, UNIX_EPOCH};
    use tokio::time::sleep;

    let interface = config
        .can_interface
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("SocketCAN requires can_interface"))?
        .to_owned();
    let base = can_input::base_id(&config);
    let mut retry = 1u64;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match CanSocket::open(&interface) {
            Ok(socket) => {
                retry = 1;
                // Kernel receive timestamps let a frame that waited in the
                // socket queue be recognised as stale instead of republished
                // as live telemetry. Not fatal when the interface lacks it.
                if let Err(error) = socket.set_recv_timestamp(true) {
                    tracing::debug!(%error, "CAN receive timestamps unavailable; using arrival time");
                }
                tracing::info!(interface, base, "Listening on CAN interface");
                {
                    let mut state = tui_state.write().await;
                    state.ecu_connected = true;
                    state.connection_address = config.connection_display();
                }
                loop {
                    let received = tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        result = socket.read_frame_with_timestamp() => result,
                    };
                    let (frame, captured) = match received {
                        Ok(pair) => pair,
                        Err(error) => {
                            tracing::warn!(%error, "CAN read failed; reopening interface");
                            break;
                        }
                    };
                    // Data frames only: remote and error frames carry no
                    // telemetry, and a short frame cannot be decoded.
                    let CanFrame::Data(data) = frame else {
                        continue;
                    };
                    if data.data().len() != 8 {
                        continue;
                    }
                    let (id, extended) = match data.id() {
                        Id::Standard(id) => (u32::from(id.as_raw()), false),
                        Id::Extended(id) => (id.as_raw(), true),
                    };
                    can_input::accept(
                        CanInputFrame {
                            id,
                            extended,
                            data: data.data().to_vec(),
                            timestamp_ms: captured
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|since| since.as_millis() as u64),
                        },
                        base,
                        &config,
                        sender.as_ref(),
                        &tui_state,
                    )
                    .await?;
                }
            }
            Err(error) => tracing::warn!(%error, interface, "Cannot open CAN interface"),
        }
        tui_state.write().await.ecu_connected = false;
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = sleep(Duration::from_secs(retry)) => {}
        }
        retry = (retry * 2).min(30);
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn run(
    config: Arc<AppConfig>,
    _sender: Option<mpsc::Sender<MqttMessage>>,
    _tui_state: Arc<RwLock<TuiState>>,
    _cancel: CancellationToken,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "connection_type = \"can\" ({}) needs Linux SocketCAN; on this platform run \
         scripts/can_gateway.py and use connection_type = \"tcp\"",
        config.connection_display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu_protocol::EcuProtocol;

    fn config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            ecu_protocol: EcuProtocol::HaltechCanV2,
            connection_type: "can".into(),
            can_interface: Some("vcan0".into()),
            mqtt_enabled: false,
            ..Default::default()
        })
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn non_linux_explains_how_to_reach_the_bus_instead() {
        let error = run(
            config(),
            None,
            Arc::new(RwLock::new(TuiState::default())),
            CancellationToken::new(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("SocketCAN"), "{error}");
        assert!(error.contains("can_gateway.py"), "{error}");
    }

    /// A missing interface must retry rather than exit, so the service can
    /// start before the CAN device is brought up.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unavailable_interface_retries_until_cancelled() {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(
            Arc::new(AppConfig {
                can_interface: Some("ecu-test-missing0".into()),
                ..(*config()).clone()
            }),
            None,
            Arc::new(RwLock::new(TuiState::default())),
            cancel.clone(),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!task.is_finished());
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    /// Requires a virtual bus: `sudo ip link add dev vcan0 type vcan &&
    /// sudo ip link set up vcan0`. Run with `cargo test -- --ignored`.
    #[cfg(target_os = "linux")]
    #[ignore = "needs a vcan0 interface"]
    #[tokio::test]
    async fn reads_live_frames_from_a_virtual_can_bus() {
        use socketcan::{CanFrame, EmbeddedFrame, Socket, StandardId};
        let (sender, mut receiver) = mpsc::channel(4);
        let state = Arc::new(RwLock::new(TuiState::default()));
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(
            config(),
            Some(sender),
            Arc::clone(&state),
            cancel.clone(),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let writer = socketcan::CanSocket::open("vcan0").expect("vcan0 must exist");
        let frame = CanFrame::new(
            StandardId::new(0x360).unwrap(),
            &[0x17, 0x70, 0x03, 0xf5, 0x02, 0xee, 0, 0],
        )
        .unwrap();
        writer.write_frame(&frame).unwrap();

        let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&message.payload).unwrap();
        assert_eq!(payload["channels"]["rpm"], 6000.);
        assert_eq!(payload["source"], "haltech-can-v2");
        assert_eq!(state.read().await.channels["rpm"].value, 6000.);
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
