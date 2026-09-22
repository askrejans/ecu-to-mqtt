//! MQTT Handler Module
//!
//! Handles asynchronous MQTT client operations including connection, authentication,
//! TLS/SSL support, message buffering, and automatic reconnection with circuit breaker pattern.

use crate::config::AppConfig;
use crate::errors::{MqttError, Result};
use paho_mqtt as mqtt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// MQTT message to be published
#[derive(Debug, Clone)]
pub struct MqttMessage {
    pub topic: String,
    pub payload: String,
    pub qos: i32,
    pub retained: bool,
}

impl MqttMessage {
    pub fn new(topic: String, payload: String, qos: i32) -> Self {
        Self {
            topic,
            payload,
            qos,
            retained: false,
        }
    }
}

/// MQTT Client Handler with buffering and reconnection logic
pub struct MqttHandler {
    client: mqtt::AsyncClient,
    config: Arc<AppConfig>,
    message_buffer: mpsc::Sender<MqttMessage>,
    buffer_receiver: Option<mpsc::Receiver<MqttMessage>>,
    is_connected: bool,
    reconnection_attempts: u32,
}

impl MqttHandler {
    /// Create a new MQTT handler
    pub fn new(config: Arc<AppConfig>) -> Result<Self> {
        let client_id = config
            .mqtt_client_id
            .clone()
            .unwrap_or_else(|| format!("ecu-to-mqtt-{}", std::process::id()));

        let server_uri = if config.mqtt_use_tls {
            format!("ssl://{}:{}", config.mqtt_host, config.mqtt_port)
        } else {
            format!("tcp://{}:{}", config.mqtt_host, config.mqtt_port)
        };

        info!("Creating MQTT client with ID: {}", client_id);
        info!("MQTT broker URI: {}", server_uri);

        let create_opts = mqtt::CreateOptionsBuilder::new()
            .server_uri(&server_uri)
            .client_id(&client_id)
            .finalize();

        let client = mqtt::AsyncClient::new(create_opts)
            .map_err(|e| MqttError::ClientCreationFailed(e.to_string()))?;

        // Create message buffer channel
        let (tx, rx) = mpsc::channel(config.message_buffer_size);

        Ok(Self {
            client,
            config,
            message_buffer: tx,
            buffer_receiver: Some(rx),
            is_connected: false,
            reconnection_attempts: 0,
        })
    }

    /// Connect to MQTT broker with authentication and TLS support
    pub async fn connect(&mut self) -> Result<()> {
        info!(
            "Connecting to MQTT broker at {}:{}",
            self.config.mqtt_host, self.config.mqtt_port
        );

        let mut conn_opts_builder = mqtt::ConnectOptionsBuilder::new();
        conn_opts_builder
            .keep_alive_interval(Duration::from_secs(30))
            .clean_session(true)
            .automatic_reconnect(Duration::from_secs(1), Duration::from_secs(60));

        // Add authentication if configured
        if let Some(ref username) = self.config.mqtt_username {
            debug!("Configuring MQTT authentication for user: {}", username);
            conn_opts_builder.user_name(username);

            if let Some(ref password) = self.config.mqtt_password {
                conn_opts_builder.password(password.as_str());
            }
        }

        // Configure TLS/SSL if enabled
        if self.config.mqtt_use_tls {
            info!("Configuring TLS/SSL for MQTT connection");

            let mut ssl_opts_builder = mqtt::SslOptionsBuilder::new();

            if let Some(ref ca_path) = self.config.mqtt_ca_cert_path {
                debug!("Using CA certificate: {}", ca_path);
                ssl_opts_builder
                    .trust_store(ca_path)
                    .map_err(|e| MqttError::TlsError(e.to_string()))?;
            }

            if let Some(ref cert_path) = self.config.mqtt_client_cert_path {
                debug!("Using client certificate: {}", cert_path);
                ssl_opts_builder
                    .key_store(cert_path)
                    .map_err(|e| MqttError::TlsError(e.to_string()))?;
            }

            if let Some(ref key_path) = self.config.mqtt_client_key_path {
                debug!("Using client private key: {}", key_path);
                ssl_opts_builder
                    .private_key(key_path)
                    .map_err(|e| MqttError::TlsError(e.to_string()))?;
            }

            let ssl_opts = ssl_opts_builder.finalize();
            conn_opts_builder.ssl_options(ssl_opts);
        }

        // Build connection options and drop the builder BEFORE awaiting so that the
        // future stays `Send` (ConnectOptionsBuilder wraps raw FFI pointers).
        let conn_opts = conn_opts_builder.finalize();
        drop(conn_opts_builder);

        // Attempt connection
        self.client
            .connect(conn_opts)
            .await
            .map_err(|e| MqttError::ConnectionFailed {
                broker: format!("{}:{}", self.config.mqtt_host, self.config.mqtt_port),
                source: e,
            })?;

        self.is_connected = true;
        self.reconnection_attempts = 0;

        info!("Successfully connected to MQTT broker");

        if self.config.mqtt_username.is_some() {
            info!("Authenticated MQTT connection established");
        }

        Ok(())
    }

    /// Publish a single message
    pub async fn publish(&self, message: &MqttMessage) -> Result<()> {
        if !self.is_connected {
            return Err(MqttError::ConnectionLost("Not connected to broker".to_string()).into());
        }

        let msg = mqtt::MessageBuilder::new()
            .topic(&message.topic)
            .payload(message.payload.as_bytes())
            .qos(message.qos)
            .retained(message.retained)
            .finalize();

        self.client
            .publish(msg)
            .await
            .map_err(|e| MqttError::PublishFailed {
                topic: message.topic.clone(),
                source: e,
            })?;

        debug!("Published to topic: {}", message.topic);
        Ok(())
    }

    /// Queue a message for publishing (non-blocking)
    #[allow(dead_code)]
    pub async fn queue_message(&self, message: MqttMessage) -> Result<()> {
        self.message_buffer
            .send(message)
            .await
            .map_err(|_| MqttError::BufferFull)?;
        Ok(())
    }

    /// Start the message publishing task (consumes buffer receiver)
    pub async fn start_publishing_task(mut self) -> Result<()> {
        let mut receiver = self.buffer_receiver.take().ok_or_else(|| {
            MqttError::ClientCreationFailed("Buffer receiver already taken".to_string())
        })?;

        info!("Starting MQTT message publishing task");

        while let Some(message) = receiver.recv().await {
            match self.publish(&message).await {
                Ok(_) => {
                    // Success - reset reconnection attempts counter
                    if self.reconnection_attempts > 0 {
                        self.reconnection_attempts = 0;
                    }
                }
                Err(e) => {
                    error!("Failed to publish message to {}: {}", message.topic, e);

                    // Attempt reconnection with exponential backoff
                    self.is_connected = false;
                    self.reconnection_attempts += 1;

                    if self.reconnection_attempts <= self.config.max_retry_count {
                        warn!(
                            "Attempting to reconnect (attempt {}/{})",
                            self.reconnection_attempts, self.config.max_retry_count
                        );

                        let delay = self.calculate_backoff_delay();
                        sleep(Duration::from_millis(delay)).await;

                        if let Err(e) = self.connect().await {
                            error!("Reconnection failed: {}", e);
                        }

                        // Retry publishing this message
                        if self.is_connected
                            && let Err(e) = self.publish(&message).await
                        {
                            error!("Retry publish failed: {}", e);
                        }
                    } else {
                        error!("Max reconnection attempts exceeded, dropping message");
                    }
                }
            }
        }

        info!("Message publishing task ended");
        Ok(())
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff_delay(&self) -> u64 {
        let base_delay = self.config.initial_retry_delay_ms;
        let max_delay = self.config.max_retry_delay_ms;
        let attempts = self.reconnection_attempts.saturating_sub(1);

        let delay = base_delay * 2_u64.pow(attempts);
        delay.min(max_delay)
    }

    /// Check if connected
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.is_connected && self.client.is_connected()
    }

    /// Disconnect from broker
    #[allow(dead_code)]
    pub async fn disconnect(&mut self) -> Result<()> {
        if self.is_connected {
            info!("Disconnecting from MQTT broker");

            self.client
                .disconnect(None)
                .await
                .map_err(MqttError::DisconnectFailed)?;

            self.is_connected = false;
            info!("Disconnected from MQTT broker");
        }
        Ok(())
    }

    /// Get a clone of the message sender for queuing messages
    pub fn get_sender(&self) -> mpsc::Sender<MqttMessage> {
        self.message_buffer.clone()
    }
}

/// Helper function to create a complete topic path
pub fn build_topic_path(base_topic: &str, sub_topic: &str) -> String {
    let base = base_topic.trim_end_matches('/');
    let sub = sub_topic.trim_start_matches('/');
    format!("{}/{}", base, sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_topic_path() {
        assert_eq!(build_topic_path("/GOLF86/ECU/", "RPM"), "/GOLF86/ECU/RPM");

        assert_eq!(build_topic_path("/GOLF86/ECU", "/RPM"), "/GOLF86/ECU/RPM");

        assert_eq!(
            build_topic_path("/test/", "/status/ready"),
            "/test/status/ready"
        );
    }

    #[test]
    fn test_mqtt_message_creation() {
        let msg = MqttMessage::new("/test/topic".to_string(), "test payload".to_string(), 1);

        assert_eq!(msg.topic, "/test/topic");
        assert_eq!(msg.payload, "test payload");
        assert_eq!(msg.qos, 1);
        assert!(!msg.retained);
    }

    #[test]
    fn test_backoff_calculation() {
        let config = AppConfig {
            initial_retry_delay_ms: 1000,
            max_retry_delay_ms: 60000,
            ..Default::default()
        };

        let handler = MqttHandler::new(Arc::new(config)).unwrap();

        // Test exponential backoff
        let mut test_handler = handler;
        test_handler.reconnection_attempts = 1;
        assert_eq!(test_handler.calculate_backoff_delay(), 1000);

        test_handler.reconnection_attempts = 2;
        assert_eq!(test_handler.calculate_backoff_delay(), 2000);

        test_handler.reconnection_attempts = 3;
        assert_eq!(test_handler.calculate_backoff_delay(), 4000);

        // Should cap at max
        test_handler.reconnection_attempts = 20;
        assert_eq!(test_handler.calculate_backoff_delay(), 60000);
    }

    #[tokio::test]
    async fn test_mqtt_handler_creation() {
        let config = Arc::new(AppConfig::default());
        let handler = MqttHandler::new(config);

        assert!(handler.is_ok());
        let handler = handler.unwrap();
        assert!(!handler.is_connected());
    }

    #[test]
    fn test_mqtt_message_qos_values() {
        let msg0 = MqttMessage::new("topic".to_string(), "data".to_string(), 0);
        let msg1 = MqttMessage::new("topic".to_string(), "data".to_string(), 1);
        let msg2 = MqttMessage::new("topic".to_string(), "data".to_string(), 2);

        assert_eq!(msg0.qos, 0);
        assert_eq!(msg1.qos, 1);
        assert_eq!(msg2.qos, 2);
    }

    #[test]
    fn test_mqtt_message_empty_payload() {
        let msg = MqttMessage::new("topic".to_string(), String::new(), 0);
        assert_eq!(msg.payload, "");
    }
}
