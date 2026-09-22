//! Error types for the ECU-to-MQTT application
//!
//! This module defines custom error types for different subsystems
//! using the thiserror crate for ergonomic error handling.

use thiserror::Error;

/// Main application error type that encompasses all subsystem errors
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Serial communication error: {0}")]
    Serial(#[from] SerialError),

    #[error("MQTT error: {0}")]
    Mqtt(#[from] MqttError),

    #[error("Data parsing error: {0}")]
    Parse(#[from] ParseError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Other error: {0}")]
    #[allow(dead_code)]
    Other(String),
}

/// Configuration-related errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to load configuration: {0}")]
    LoadFailed(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid value for '{field}': {message}")]
    InvalidValue { field: String, message: String },

    #[error("Validation failed: {0}")]
    #[allow(dead_code)]
    ValidationFailed(String),

    #[error("Configuration file not found at any expected location")]
    #[allow(dead_code)]
    NotFound,
}

/// Serial port / TCP connection errors
#[derive(Error, Debug)]
pub enum SerialError {
    #[error("Failed to open serial port '{port}': {source}")]
    OpenFailed {
        port: String,
        source: tokio_serial::Error,
    },

    #[error("Failed to connect to TCP endpoint '{addr}': {source}")]
    TcpConnectFailed {
        addr: String,
        source: std::io::Error,
    },

    #[error("Failed to configure serial port: {0}")]
    #[allow(dead_code)]
    ConfigFailed(tokio_serial::Error),

    #[error("Read timeout after {timeout_ms}ms")]
    ReadTimeout { timeout_ms: u64 },

    #[error("Write failed: {0}")]
    WriteFailed(std::io::Error),

    #[error("Read failed: {0}")]
    ReadFailed(std::io::Error),

    #[error("Device disconnected")]
    Disconnected,

    #[error("Invalid response: expected {expected} bytes, got {actual}")]
    #[allow(dead_code)]
    InvalidResponse { expected: usize, actual: usize },

    #[error("Maximum reconnection attempts ({0}) exceeded")]
    MaxRetriesExceeded(u32),
}

/// MQTT client errors
#[derive(Error, Debug)]
pub enum MqttError {
    #[error("Failed to create MQTT client: {0}")]
    ClientCreationFailed(String),

    #[error("Failed to connect to broker at {broker}: {source}")]
    ConnectionFailed {
        broker: String,
        source: paho_mqtt::Error,
    },

    #[error("Failed to publish message to topic '{topic}': {source}")]
    PublishFailed {
        topic: String,
        source: paho_mqtt::Error,
    },

    #[error("Failed to disconnect: {0}")]
    #[allow(dead_code)]
    DisconnectFailed(paho_mqtt::Error),

    #[error("Connection lost: {0}")]
    ConnectionLost(String),

    #[error("TLS/SSL error: {0}")]
    TlsError(String),

    #[error("Authentication failed")]
    #[allow(dead_code)]
    AuthenticationFailed,

    #[error("Message buffer full, dropping message")]
    #[allow(dead_code)]
    BufferFull,
}

/// Data parsing errors
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Insufficient data: expected at least {expected} bytes, got {actual}")]
    InsufficientData { expected: usize, actual: usize },

    #[error("Invalid data at offset {offset}: {message}")]
    InvalidData { offset: usize, message: String },

    #[error("Checksum mismatch: expected {expected:x}, got {actual:x}")]
    #[allow(dead_code)]
    ChecksumMismatch { expected: u8, actual: u8 },

    #[error("Data validation failed for '{field}': value {value} is out of range [{min}, {max}]")]
    #[allow(dead_code)]
    ValidationFailed {
        field: String,
        value: f64,
        min: f64,
        max: f64,
    },
}

/// Result type alias for application operations
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ConfigError::MissingField("port_name".to_string());
        assert_eq!(err.to_string(), "Missing required field: port_name");

        let err = ParseError::ValidationFailed {
            field: "RPM".to_string(),
            value: 20000.0,
            min: 0.0,
            max: 15000.0,
        };
        assert!(err.to_string().contains("RPM"));
        assert!(err.to_string().contains("20000"));
    }

    #[test]
    fn test_error_conversion() {
        let config_err = ConfigError::NotFound;
        let app_err: AppError = config_err.into();
        assert!(matches!(app_err, AppError::Config(_)));
    }

    #[test]
    fn test_result_type() {
        fn sample_fn() -> Result<i32> {
            Ok(42)
        }
        assert_eq!(sample_fn().unwrap(), 42);
    }
}
