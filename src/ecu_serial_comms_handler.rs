//! ECU Communication Handler
//!
//! Handles async communication with the Speeduino ECU over either a hardware
//! serial port or a raw TCP socket (e.g. an ESP32 / Moxa WiFi bridge).
//! Implements exponential backoff for reconnection and robust error handling.

use crate::config::AppConfig;
use crate::connection::EcuConnection;
use crate::errors::{Result, SerialError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Time to wait after sending the command before draining the buffer.
/// At 115200 baud, 138 bytes take ~12 ms to transmit.  150 ms gives
/// comfortable headroom for any Speeduino firmware version so the entire
/// packet is sitting in the OS buffer before we start reading.
const COMMAND_PROCESSING_DELAY_MS: u64 = 150;

/// Maximum response buffer size — larger than any known Speeduino packet.
const MAX_PACKET_BYTES: usize = 256;

/// ECU connection handler — supports hardware serial and TCP (e.g. WiFi bridge).
pub struct EcuSerialHandler {
    connection: Option<EcuConnection>,
    config: AppConfig,
    retry_count: u32,
    current_delay_ms: u64,
}

impl EcuSerialHandler {
    /// Create a new ECU serial handler
    pub fn new(config: AppConfig) -> Self {
        Self {
            connection: None,
            config: config.clone(),
            retry_count: 0,
            current_delay_ms: config.initial_retry_delay_ms,
        }
    }

    /// Open the ECU connection (serial or TCP depending on config).
    pub async fn connect(&mut self) -> Result<()> {
        info!("Connecting to ECU: {}", self.config.connection_display());

        let conn = EcuConnection::open(&self.config).await?;
        self.connection = Some(conn);
        self.retry_count = 0;
        self.current_delay_ms = self.config.initial_retry_delay_ms;

        info!("Successfully connected to ECU");
        Ok(())
    }

    /// Returns `true` when an active connection exists.
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Close the ECU connection.
    pub async fn disconnect(&mut self) {
        if self.connection.take().is_some() {
            debug!("ECU connection closed");
        }
    }

    /// Read engine data from the ECU
    ///
    /// Speeduino retains its variable-length response. MegaSquirt's fixed
    /// compatibility response is read to completion under one bounded deadline,
    /// including when a serial-to-TCP adapter fragments it between packets.
    pub async fn read_engine_data(&mut self) -> Result<Vec<u8>> {
        let conn = self.connection.as_mut().ok_or(SerialError::Disconnected)?;

        // ── Flush hardware buffer before sending ──────────────────────────────
        conn.clear_buffers().ok();

        // ── Send command ──────────────────────────────────────────────────────
        let command = self.config.ecu_protocol.command();
        debug!("Sending telemetry read command: {:?}", command);
        conn.write_all(command)
            .await
            .map_err(SerialError::WriteFailed)?;
        conn.flush().await.map_err(SerialError::WriteFailed)?;

        if self.config.ecu_protocol != crate::ecu_protocol::EcuProtocol::Speeduino {
            let mut buffer = vec![0; 112];
            let result = timeout(
                Duration::from_millis(self.config.read_timeout_ms),
                conn.read_exact(&mut buffer),
            )
            .await;
            match result {
                Ok(Ok(_)) => return Ok(buffer),
                Ok(Err(error)) => {
                    // A partial packet cannot become the next response.
                    self.connection = None;
                    return Err(SerialError::ReadFailed(error).into());
                }
                Err(_) => {
                    self.connection = None;
                    return Err(SerialError::ReadTimeout {
                        timeout_ms: self.config.read_timeout_ms,
                    }
                    .into());
                }
            }
        }

        // ── Wait for ECU to finish transmitting ───────────────────────────────
        sleep(Duration::from_millis(COMMAND_PROCESSING_DELAY_MS)).await;

        // ── Drain whatever is in the OS buffer ────────────────────────────────
        // For serial: all bytes arrived during the sleep above, so every read()
        // returns immediately.  For TCP: the first read() uses the full deadline
        // in case the response is still in-flight (TCP has no UART buffer
        // guarantee).  Subsequent reads use 5 ms to drain any remaining bytes
        // without blocking once the bus goes idle.
        let mut buffer: Vec<u8> = Vec::with_capacity(MAX_PACKET_BYTES);
        let first_deadline = Duration::from_millis(self.config.read_timeout_ms);
        let mut first_read = true;
        loop {
            let per_read_timeout = if first_read {
                first_deadline
            } else {
                Duration::from_millis(5)
            };
            let mut tmp = [0u8; 64];
            match timeout(per_read_timeout, conn.read(&mut tmp)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    first_read = false;
                    buffer.extend_from_slice(&tmp[..n]);
                    if buffer.len() >= MAX_PACKET_BYTES {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(SerialError::ReadFailed(e).into()),
                Err(_) => break, // idle — buffer fully drained
            }
        }

        if buffer.is_empty() {
            warn!("No data received from ECU");
            return Err(SerialError::ReadTimeout {
                timeout_ms: self.config.read_timeout_ms,
            }
            .into());
        }

        debug!("Received {} bytes from ECU", buffer.len());
        Ok(buffer)
    }

    /// Attempt to reconnect with exponential backoff
    ///
    /// Returns Ok(()) if reconnection succeeded, Err if max retries exceeded
    pub async fn reconnect(&mut self) -> Result<()> {
        if self.retry_count >= self.config.max_retry_count {
            error!(
                "Maximum reconnection attempts ({}) exceeded",
                self.config.max_retry_count
            );
            return Err(SerialError::MaxRetriesExceeded(self.config.max_retry_count).into());
        }

        self.retry_count += 1;

        warn!(
            "Attempting reconnection {}/{} after {}ms delay",
            self.retry_count, self.config.max_retry_count, self.current_delay_ms
        );

        // Disconnect if still connected
        self.disconnect().await;

        // Wait with exponential backoff
        sleep(Duration::from_millis(self.current_delay_ms)).await;

        // Try to reconnect
        match self.connect().await {
            Ok(_) => {
                info!("Reconnection successful");
                self.retry_count = 0;
                self.current_delay_ms = self.config.initial_retry_delay_ms;
                Ok(())
            }
            Err(e) => {
                // Increase delay for next attempt (exponential backoff)
                self.current_delay_ms =
                    (self.current_delay_ms * 2).min(self.config.max_retry_delay_ms);

                warn!("Reconnection attempt failed: {}", e);
                Err(e)
            }
        }
    }

    /// Check whether the connection target exists / is reachable.
    ///
    /// For TCP mode, always returns `true` (TCP connectivity is determined at
    /// connect time).  For serial, walks the available-ports list.
    pub fn check_device_exists(&self) -> bool {
        if self.config.connection_type == "tcp" {
            return true;
        }
        match tokio_serial::available_ports() {
            Ok(ports) => {
                let exists = ports.iter().any(|p| p.port_name == self.config.port_name);
                if !exists {
                    debug!(
                        "Device {} not found in available ports",
                        self.config.port_name
                    );
                }
                exists
            }
            Err(e) => {
                warn!("Failed to enumerate serial ports: {}", e);
                false
            }
        }
    }

    /// Reset retry counter (call after successful operations)
    pub fn reset_retry_count(&mut self) {
        if self.retry_count > 0 {
            debug!("Resetting retry counter");
            self.retry_count = 0;
            self.current_delay_ms = self.config.initial_retry_delay_ms;
        }
    }

    /// Get current retry attempt number
    pub fn get_retry_count(&self) -> u32 {
        self.retry_count
    }
}

impl Drop for EcuSerialHandler {
    fn drop(&mut self) {
        debug!("Dropping EcuSerialHandler (connection will close automatically)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn partial_megasquirt_response_times_out_and_discards_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut handler = EcuSerialHandler::new(AppConfig {
            ecu_protocol: crate::ecu_protocol::EcuProtocol::Ms3,
            connection_type: "tcp".into(),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(listener.local_addr().unwrap().port()),
            read_timeout_ms: 40,
            ..Default::default()
        });
        handler.connect().await.unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let read = tokio::spawn(async move {
            assert!(handler.read_engine_data().await.is_err());
            assert!(!handler.is_connected());
        });
        let mut command = [0; 3];
        stream.read_exact(&mut command).await.unwrap();
        stream.write_all(&[0; 37]).await.unwrap();
        timeout(Duration::from_secs(1), read)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn test_ecu_serial_handler_creation() {
        let config = AppConfig::default();
        let handler = EcuSerialHandler::new(config);

        assert!(!handler.is_connected());
        assert_eq!(handler.get_retry_count(), 0);
    }

    #[test]
    fn test_retry_count_management() {
        let config = AppConfig::default();
        let mut handler = EcuSerialHandler::new(config);

        handler.retry_count = 5;
        assert_eq!(handler.get_retry_count(), 5);

        handler.reset_retry_count();
        assert_eq!(handler.get_retry_count(), 0);
    }

    #[test]
    fn test_disconnected_state() {
        let config = AppConfig::default();
        let mut handler = EcuSerialHandler::new(config);

        // Should fail when not connected
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let result = handler.read_engine_data().await;
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        let config = AppConfig {
            initial_retry_delay_ms: 1000,
            max_retry_delay_ms: 60000,
            ..Default::default()
        };

        let mut handler = EcuSerialHandler::new(config);

        // Simulate backoff growth
        assert_eq!(handler.current_delay_ms, 1000);

        handler.current_delay_ms =
            (handler.current_delay_ms * 2).min(handler.config.max_retry_delay_ms);
        assert_eq!(handler.current_delay_ms, 2000);

        handler.current_delay_ms =
            (handler.current_delay_ms * 2).min(handler.config.max_retry_delay_ms);
        assert_eq!(handler.current_delay_ms, 4000);

        // Should cap at max
        for _ in 0..10 {
            handler.current_delay_ms =
                (handler.current_delay_ms * 2).min(handler.config.max_retry_delay_ms);
        }
        assert_eq!(handler.current_delay_ms, 60000);
    }

    #[tokio::test]
    async fn test_device_check() {
        let config = AppConfig::default();
        let handler = EcuSerialHandler::new(config);

        // This will check real system ports
        // Just ensure it doesn't panic
        let _ = handler.check_device_exists();
    }
}
