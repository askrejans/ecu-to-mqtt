//! Bounded CAN frame transport from a USB/CAN gateway over TCP JSON-lines.
//! Each line is {"id":1512,"data":[...eight bytes...]}. No ECU writes occur.
use crate::{
    config::AppConfig,
    ecu_protocol::{CanInputFrame, decode_can},
    mqtt_handler::MqttMessage,
    telemetry_frame,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpStream,
    sync::mpsc,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;

pub async fn run(
    config: Arc<AppConfig>,
    sender: Option<mpsc::Sender<MqttMessage>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let host = config
        .tcp_host
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("CAN gateway requires tcp_host"))?;
    let port = config
        .tcp_port
        .ok_or_else(|| anyhow::anyhow!("CAN gateway requires tcp_port"))?;
    let base = config
        .can_base_id
        .unwrap_or(config.ecu_protocol.default_can_base());
    let mut retry = 1u64;
    loop {
        let connected = tokio::select! {
            _=cancel.cancelled()=>return Ok(()),
            result=timeout(Duration::from_secs(10),TcpStream::connect((host,port)))=>result,
        };
        if let Ok(Ok(stream)) = connected {
            retry = 1;
            let mut reader = BufReader::new(stream);
            let mut line = Vec::with_capacity(256);
            loop {
                // fill_buf/consume bounds memory even if a peer never sends a newline.
                let bytes = tokio::select! {
                    _=cancel.cancelled()=>return Ok(()),
                    result=timeout(Duration::from_secs(5),reader.fill_buf())=>result,
                };
                let Ok(Ok(bytes)) = bytes else { break };
                if bytes.is_empty() {
                    break;
                }
                let count = bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(bytes.len(), |i| i + 1);
                if line.len() + count > 4096 {
                    tracing::warn!("Oversized CAN gateway frame; reconnecting");
                    break;
                }
                line.extend_from_slice(&bytes[..count]);
                let complete = line.last() == Some(&b'\n');
                reader.consume(count);
                if !complete {
                    continue;
                }
                match serde_json::from_slice::<CanInputFrame>(&line) {
                    Ok(frame) => {
                        if let Some(captured) = frame.timestamp_ms {
                            let now =
                                SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
                            if now.abs_diff(captured) > 2000 {
                                line.clear();
                                continue;
                            }
                        }
                        match decode_can(config.ecu_protocol, base, &frame) {
                            Ok(channels) if !channels.is_empty() => {
                                telemetry_frame::publish_at(
                                    channels,
                                    config.ecu_protocol.source(),
                                    true,
                                    &config,
                                    sender.as_ref(),
                                    frame.timestamp_ms,
                                )
                                .await?
                            }
                            Ok(_) => {}
                            Err(error) => tracing::warn!(%error,"CAN frame rejected"),
                        }
                    }
                    Err(error) => tracing::warn!(%error,"Invalid CAN gateway JSON"),
                }
                line.clear();
            }
        }
        tracing::warn!(retry_seconds = retry, "CAN gateway disconnected");
        tokio::select! {_=cancel.cancelled()=>return Ok(()), _=sleep(Duration::from_secs(retry))=>{}}
        retry = (retry * 2).min(30);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu_protocol::EcuProtocol;
    use tokio::{io::AsyncWriteExt, net::TcpListener};

    #[tokio::test]
    async fn fragmented_gateway_frames_reach_mqtt_without_refreshing_stale_data() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(AppConfig {
            connection_type: "tcp".into(),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(port),
            ecu_protocol: EcuProtocol::HaltechCanV2,
            mqtt_base_topic: "/test/car/ecu/".into(),
            ..Default::default()
        });
        let (sender, mut receiver) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(config, Some(sender), cancel.clone()));
        let (mut stream, _) = listener.accept().await.unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
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
        let message = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.topic, "/test/car/ecu/telemetry");
        assert!(!message.retained);
        let payload: serde_json::Value = serde_json::from_str(&message.payload).unwrap();
        assert_eq!(payload["source"], "haltech-can-v2");
        assert_eq!(payload["timestampMs"], now);
        assert_eq!(payload["channels"]["rpm"], 6000.);
        assert_eq!(payload["partial"], true);
        assert!(receiver.try_recv().is_err());
        cancel.cancel();
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
