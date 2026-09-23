//! Configuration management module
//!
//! Handles loading, validation, and environment variable overrides for application configuration.
//! Supports `.env` files via dotenvy, TOML config files, and `ECU_TO_MQTT_*` env var overrides.

use crate::errors::{ConfigError, Result};
use config::{Config, Environment, File};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

/// Valid baud rates for serial communication
const VALID_BAUD_RATES: &[u32] = &[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

/// Prefix for environment variable overrides, e.g. `ECU_TO_MQTT_MQTT_HOST`.
pub const ENV_PREFIX: &str = "ECU_TO_MQTT";

/// Main application configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Optional absolute path for synced canonical NDJSON capture.
    #[serde(default)]
    pub telemetry_log: Option<String>,
    /// Read-only ECU protocol. See ECU_PROTOCOLS.md.
    #[serde(default)]
    pub ecu_protocol: crate::ecu_protocol::EcuProtocol,
    /// Override the profile CAN base identifier; frame format is profile-specific.
    #[serde(default)]
    pub can_base_id: Option<u32>,
    // --- Connection type ---
    /// Connection type: "serial" (hardware UART), "tcp" (raw TCP socket / WiFi
    /// bridge / CAN gateway) or "can" (native SocketCAN interface, Linux only)
    #[serde(default = "default_connection_type")]
    pub connection_type: String,

    /// SocketCAN interface for `connection_type = "can"`, e.g. "can0" or "vcan0".
    pub can_interface: Option<String>,

    /// TCP host for "tcp" connection_type (e.g. "192.168.1.100" or WiFi bridge hostname)
    pub tcp_host: Option<String>,

    /// TCP port for "tcp" connection_type (e.g. 4096 for most serial-to-TCP bridges)
    pub tcp_port: Option<u16>,

    // --- Serial port configuration (used when connection_type = "serial") ---
    /// The serial port device path (e.g., "/dev/ttyACM0", "COM3")
    #[serde(default = "default_port_name")]
    pub port_name: String,

    /// Baud rate for serial communication with Speeduino ECU
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,

    /// Expected data packet length from ECU in bytes.
    /// 130 = speeduino-serial-sim / older firmware, 138 = current Speeduino firmware (LOG_ENTRY_SIZE).
    #[serde(default = "default_expected_data_length")]
    pub expected_data_length: usize,

    /// Serial read timeout in milliseconds
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,

    // --- MQTT broker configuration ---
    /// Enable MQTT publishing (set false to run in display-only / TUI mode)
    #[serde(default = "default_mqtt_enabled")]
    pub mqtt_enabled: bool,

    /// MQTT broker host address
    #[serde(default = "default_mqtt_host")]
    pub mqtt_host: String,

    /// MQTT broker port number
    #[serde(default = "default_mqtt_port")]
    pub mqtt_port: u16,

    /// Base MQTT topic prefix (e.g. "/ECU/")
    #[serde(default = "default_mqtt_base_topic")]
    pub mqtt_base_topic: String,

    /// MQTT Quality of Service level (0, 1, or 2)
    #[serde(default = "default_mqtt_qos")]
    pub mqtt_qos: i32,

    /// MQTT client ID (auto-generated if not specified)
    pub mqtt_client_id: Option<String>,

    /// MQTT username for authentication (optional)
    pub mqtt_username: Option<String>,

    /// MQTT password for authentication (optional)
    pub mqtt_password: Option<String>,

    /// Enable MQTT over TLS/SSL
    #[serde(default)]
    pub mqtt_use_tls: bool,

    /// Path to CA certificate for TLS (optional)
    pub mqtt_ca_cert_path: Option<String>,

    /// Path to client certificate for TLS (optional)
    pub mqtt_client_cert_path: Option<String>,

    /// Path to client private key for TLS (optional)
    pub mqtt_client_key_path: Option<String>,

    // --- Application behaviour ---
    /// ECU data polling interval in milliseconds
    #[serde(default = "default_refresh_rate_ms")]
    pub refresh_rate_ms: u64,

    /// Maximum number of reconnection attempts before exiting
    #[serde(default = "default_max_retry_count")]
    pub max_retry_count: u32,

    /// Initial reconnection delay in milliseconds (exponential backoff base)
    #[serde(default = "default_initial_retry_delay_ms")]
    pub initial_retry_delay_ms: u64,

    /// Maximum reconnection delay in milliseconds (caps exponential backoff)
    #[serde(default = "default_max_retry_delay_ms")]
    pub max_retry_delay_ms: u64,

    /// MQTT message buffer size (number of messages queued when broker is unavailable)
    #[serde(default = "default_message_buffer_size")]
    pub message_buffer_size: usize,

    // --- Logging ---
    /// Log level: trace | debug | info | warn | error
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Enable JSON structured logs (useful for log aggregation systems like ELK)
    #[serde(default)]
    pub log_json: bool,

    /// Internal: path to the configuration file that was loaded (set at runtime)
    #[serde(skip)]
    pub config_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Default value functions
// ---------------------------------------------------------------------------
fn default_connection_type() -> String {
    "serial".to_string()
}
fn default_port_name() -> String {
    "/dev/ttyACM0".to_string()
}
fn default_baud_rate() -> u32 {
    115200
}
fn default_expected_data_length() -> usize {
    130
}
fn default_read_timeout_ms() -> u64 {
    2000
}
fn default_mqtt_enabled() -> bool {
    true
}
fn default_mqtt_host() -> String {
    "localhost".to_string()
}
fn default_mqtt_port() -> u16 {
    1883
}
fn default_mqtt_base_topic() -> String {
    "/ECU/".to_string()
}
fn default_mqtt_qos() -> i32 {
    0
}
fn default_refresh_rate_ms() -> u64 {
    20
}
fn default_max_retry_count() -> u32 {
    10
}
fn default_initial_retry_delay_ms() -> u64 {
    1000
}
fn default_max_retry_delay_ms() -> u64 {
    60000
}
fn default_message_buffer_size() -> usize {
    1000
}
fn default_log_level() -> String {
    "info".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            telemetry_log: None,
            ecu_protocol: crate::ecu_protocol::EcuProtocol::default(),
            can_base_id: None,
            connection_type: default_connection_type(),
            can_interface: None,
            tcp_host: None,
            tcp_port: None,
            port_name: default_port_name(),
            baud_rate: default_baud_rate(),
            expected_data_length: default_expected_data_length(),
            read_timeout_ms: default_read_timeout_ms(),
            mqtt_enabled: default_mqtt_enabled(),
            mqtt_host: default_mqtt_host(),
            mqtt_port: default_mqtt_port(),
            mqtt_base_topic: default_mqtt_base_topic(),
            mqtt_qos: default_mqtt_qos(),
            mqtt_client_id: None,
            mqtt_username: None,
            mqtt_password: None,
            mqtt_use_tls: false,
            mqtt_ca_cert_path: None,
            mqtt_client_cert_path: None,
            mqtt_client_key_path: None,
            refresh_rate_ms: default_refresh_rate_ms(),
            max_retry_count: default_max_retry_count(),
            initial_retry_delay_ms: default_initial_retry_delay_ms(),
            max_retry_delay_ms: default_max_retry_delay_ms(),
            message_buffer_size: default_message_buffer_size(),
            log_level: default_log_level(),
            log_json: false,
            config_path: None,
        }
    }
}

impl AppConfig {
    /// Validate all configuration values against acceptable ranges and constraints.
    pub fn validate(&self) -> Result<()> {
        if self
            .telemetry_log
            .as_ref()
            .is_some_and(|p| !std::path::Path::new(p).is_absolute())
        {
            return Err(ConfigError::ValidationFailed(
                "telemetry_log must be an absolute path".into(),
            )
            .into());
        }
        debug!("Validating configuration");
        let connection = self.connection_type.to_lowercase();
        if self.ecu_protocol.is_can() && connection != "can" && connection != "tcp" {
            return Err(ConfigError::ValidationFailed(
                "CAN profiles require connection_type = \"can\" (SocketCAN) or \"tcp\" (CAN gateway)"
                    .into(),
            )
            .into());
        }
        if !self.ecu_protocol.is_can() && connection == "can" {
            return Err(ConfigError::ValidationFailed(format!(
                "connection_type = \"can\" needs a CAN profile; {} is a serial protocol",
                self.ecu_protocol.name()
            ))
            .into());
        }
        if self.can_base_id.is_some_and(|id| {
            id.checked_add(self.ecu_protocol.last_can_offset())
                .is_none_or(|last| {
                    last > if self.ecu_protocol.extended() {
                        0x1fffffff
                    } else {
                        0x7ff
                    }
                })
        }) {
            return Err(ConfigError::ValidationFailed(
                "CAN base identifier leaves no room for broadcast groups".into(),
            )
            .into());
        }

        match connection.as_str() {
            "can" => {
                if self
                    .can_interface
                    .as_deref()
                    .map(|name| name.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(ConfigError::MissingField(
                        "can_interface (required when connection_type = \"can\")".to_string(),
                    )
                    .into());
                }
            }
            "serial" => {
                if self.port_name.is_empty() {
                    return Err(ConfigError::MissingField("port_name".to_string()).into());
                }
                if !VALID_BAUD_RATES.contains(&self.baud_rate) {
                    return Err(ConfigError::InvalidValue {
                        field: "baud_rate".to_string(),
                        message: format!("must be one of: {:?}", VALID_BAUD_RATES),
                    }
                    .into());
                }
            }
            "tcp" => {
                if self
                    .tcp_host
                    .as_deref()
                    .map(|h| h.is_empty())
                    .unwrap_or(true)
                {
                    return Err(ConfigError::MissingField(
                        "tcp_host (required when connection_type = \"tcp\")".to_string(),
                    )
                    .into());
                }
                if self.tcp_port.is_none() {
                    return Err(ConfigError::MissingField(
                        "tcp_port (required when connection_type = \"tcp\")".to_string(),
                    )
                    .into());
                }
            }
            other => {
                return Err(ConfigError::InvalidValue {
                    field: "connection_type".to_string(),
                    message: format!("must be \"serial\", \"tcp\" or \"can\", got \"{}\"", other),
                }
                .into());
            }
        }

        if self.ecu_protocol == crate::ecu_protocol::EcuProtocol::Speeduino
            && (self.expected_data_length < 119 || self.expected_data_length > 256)
        {
            return Err(ConfigError::InvalidValue {
                field: "expected_data_length".to_string(),
                message: "must be between 119 and 256".to_string(),
            }
            .into());
        }

        if self.read_timeout_ms == 0 || self.read_timeout_ms > 30000 {
            return Err(ConfigError::InvalidValue {
                field: "read_timeout_ms".to_string(),
                message: "must be between 1 and 30000 milliseconds".to_string(),
            }
            .into());
        }

        if self.mqtt_enabled {
            if self.mqtt_host.is_empty() {
                return Err(ConfigError::MissingField("mqtt_host".to_string()).into());
            }
            if self.mqtt_port == 0 {
                return Err(ConfigError::InvalidValue {
                    field: "mqtt_port".to_string(),
                    message: "must be greater than 0".to_string(),
                }
                .into());
            }
            if self.mqtt_base_topic.is_empty() {
                return Err(ConfigError::MissingField("mqtt_base_topic".to_string()).into());
            }
            if !(0..=2).contains(&self.mqtt_qos) {
                return Err(ConfigError::InvalidValue {
                    field: "mqtt_qos".to_string(),
                    message: "must be 0, 1, or 2".to_string(),
                }
                .into());
            }
            if self.mqtt_use_tls
                && let Some(ref ca_path) = self.mqtt_ca_cert_path
                && !Path::new(ca_path).exists()
            {
                return Err(ConfigError::InvalidValue {
                    field: "mqtt_ca_cert_path".to_string(),
                    message: format!("file does not exist: {}", ca_path),
                }
                .into());
            }
            if self.message_buffer_size == 0 {
                return Err(ConfigError::InvalidValue {
                    field: "message_buffer_size".to_string(),
                    message: "must be greater than 0".to_string(),
                }
                .into());
            }
        }

        if self.refresh_rate_ms == 0 || self.refresh_rate_ms > 10000 {
            return Err(ConfigError::InvalidValue {
                field: "refresh_rate_ms".to_string(),
                message: "must be between 1 and 10000 milliseconds".to_string(),
            }
            .into());
        }

        if self.max_retry_count == 0 {
            return Err(ConfigError::InvalidValue {
                field: "max_retry_count".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }

        if self.initial_retry_delay_ms == 0 {
            return Err(ConfigError::InvalidValue {
                field: "initial_retry_delay_ms".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }

        if self.max_retry_delay_ms < self.initial_retry_delay_ms {
            return Err(ConfigError::InvalidValue {
                field: "max_retry_delay_ms".to_string(),
                message: "must be >= initial_retry_delay_ms".to_string(),
            }
            .into());
        }

        let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_log_levels.contains(&self.log_level.to_lowercase().as_str()) {
            return Err(ConfigError::InvalidValue {
                field: "log_level".to_string(),
                message: format!("must be one of: {:?}", valid_log_levels),
            }
            .into());
        }

        info!("Configuration validation successful");
        Ok(())
    }

    /// Returns a human-readable description of the ECU connection endpoint.
    pub fn connection_display(&self) -> String {
        match self.connection_type.to_lowercase().as_str() {
            "can" => format!("CAN {}", self.can_interface.as_deref().unwrap_or("?")),
            "tcp" => format!(
                "TCP {}:{}",
                self.tcp_host.as_deref().unwrap_or("?"),
                self.tcp_port.unwrap_or(0)
            ),
            _ => format!("{} @ {} baud", self.port_name, self.baud_rate),
        }
    }

    /// Log a sanitised configuration summary (passwords/keys are redacted).
    #[allow(dead_code)]
    pub fn display_summary(&self) {
        info!("=== Configuration Summary ===");
        info!(
            "Connection: {} ({})",
            self.connection_type,
            self.connection_display()
        );
        if self.mqtt_enabled {
            info!("MQTT Broker: {}:{}", self.mqtt_host, self.mqtt_port);
            info!("MQTT Base Topic: {}", self.mqtt_base_topic);
            info!("MQTT QoS: {}", self.mqtt_qos);
            if self.mqtt_use_tls {
                info!("MQTT TLS: enabled");
            }
            if self.mqtt_username.is_some() {
                info!("MQTT Auth: enabled (credentials redacted)");
            }
        } else {
            info!("MQTT: disabled (display-only / TUI mode)");
        }
        info!("Refresh Rate: {}ms", self.refresh_rate_ms);
        info!("Max Retry Count: {}", self.max_retry_count);
        info!("Log Level: {}", self.log_level);
        info!("============================");
    }
}

/// Environment override source. `overrides` replaces the process environment in
/// tests; `None` reads the real environment.
fn env_source(overrides: Option<HashMap<String, String>>) -> Environment {
    // No separator: ECU_TO_MQTT_MQTT_ENABLED → "mqtt_enabled" (flat key).
    // With separator("_") the crate converts underscores to dots producing
    // nested keys like "mqtt.enabled" which don't match the flat struct fields.
    Environment::with_prefix(ENV_PREFIX)
        .try_parsing(true)
        .source(overrides)
}

/// Config file locations, lowest priority first.
fn candidate_paths() -> Vec<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = [
        "/usr/etc/g86-car-telemetry/ecu-to-mqtt.toml",
        "/etc/g86-car-telemetry/ecu-to-mqtt.toml",
        "/etc/ecu-to-mqtt/settings.toml",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .collect();

    // Directory containing the executable (covers `cargo install` and
    // installed service binaries).
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("settings.toml"));
        candidates.push(parent.join("ecu-to-mqtt.toml"));
    }

    // Current working directory (highest priority — developer / manual run).
    for name in ["./settings.toml", "./ecu-to-mqtt.toml"] {
        candidates.push(std::path::PathBuf::from(name));
    }
    candidates
}

/// Load application configuration from `.env`, TOML files, and environment variables.
///
/// Priority (highest to lowest):
/// 1. `ECU_TO_MQTT_*` environment variables
/// 2. Specified config file (via `config_path` argument)
/// 3. Default config file locations
/// 4. Built-in defaults
pub fn load_configuration(config_path: Option<&str>) -> Result<AppConfig> {
    // Load .env file if present (silently ignored when missing)
    dotenvy::dotenv().ok();

    info!("Loading configuration");

    let mut builder = Config::builder();

    let loaded_from = if let Some(path) = config_path {
        debug!("Loading config from specified path: {}", path);
        builder = builder.add_source(File::with_name(path).required(true));
        Some(path.to_string())
    } else {
        // Earlier entries have lower priority (later sources override earlier ones
        // in the `config` crate), so list most-specific last.
        let candidates = candidate_paths();

        let mut found_path = None;
        for path in &candidates {
            if path.exists() {
                debug!("Found config at: {:?}", path);
                builder = builder.add_source(File::from(path.clone()).required(false));
                found_path = Some(path.display().to_string());
            }
        }

        if found_path.is_none() {
            warn!("No configuration file found; using defaults and environment variables");
        }

        found_path
    };

    builder = builder.add_source(env_source(None));

    let settings = builder
        .build()
        .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

    let mut app_config: AppConfig = settings
        .try_deserialize()
        .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

    app_config.config_path = loaded_from;
    app_config.validate()?;

    match &app_config.config_path {
        Some(path) => info!("Configuration loaded from: {}", path),
        None => info!("Configuration loaded from defaults and environment"),
    }

    Ok(app_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ecu_protocol::EcuProtocol;
    use std::fs;
    use tempfile::tempdir;

    fn parse_toml(content: &str) -> AppConfig {
        Config::builder()
            .add_source(File::from_str(content, config::FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    /// Every shipped example must load and validate, so a new demo config
    /// cannot ship with a profile/connection combination the binary rejects.
    #[test]
    fn shipped_ecu_examples_are_valid() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut checked = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let config = parse_toml(&fs::read_to_string(&path).unwrap());
            config
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            checked += 1;
        }
        assert!(
            checked >= 15,
            "expected one example per profile, got {checked}"
        );
    }

    /// One example per supported profile, so every ECU has a demo config.
    #[test]
    fn every_profile_has_an_example_config() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut seen: Vec<EcuProtocol> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                (path.extension().and_then(|e| e.to_str()) == Some("toml"))
                    .then(|| parse_toml(&fs::read_to_string(&path).unwrap()).ecu_protocol)
            })
            .collect();
        for protocol in EcuProtocol::ALL {
            assert!(
                seen.contains(&protocol),
                "no example config uses {}",
                protocol.name()
            );
        }
        seen.sort_by_key(|p| p.name());
        seen.dedup();
        assert_eq!(seen.len(), EcuProtocol::ALL.len());
    }

    #[test]
    fn environment_variables_override_every_field() {
        let overrides = HashMap::from([
            ("ECU_TO_MQTT_CONNECTION_TYPE".to_string(), "tcp".to_string()),
            (
                "ECU_TO_MQTT_TCP_HOST".to_string(),
                "192.168.1.100".to_string(),
            ),
            ("ECU_TO_MQTT_TCP_PORT".to_string(), "29536".to_string()),
            (
                "ECU_TO_MQTT_ECU_PROTOCOL".to_string(),
                "haltech_can_v2".to_string(),
            ),
            ("ECU_TO_MQTT_MQTT_ENABLED".to_string(), "false".to_string()),
            ("ECU_TO_MQTT_CAN_BASE_ID".to_string(), "864".to_string()),
        ]);
        let config: AppConfig = Config::builder()
            .add_source(env_source(Some(overrides)))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(config.ecu_protocol, EcuProtocol::HaltechCanV2);
        assert_eq!(config.tcp_host.as_deref(), Some("192.168.1.100"));
        assert_eq!(config.tcp_port, Some(29536));
        assert_eq!(config.can_base_id, Some(0x360));
        assert!(!config.mqtt_enabled);
        config.validate().unwrap();
    }

    #[test]
    fn environment_variables_take_priority_over_the_config_file() {
        let config: AppConfig = Config::builder()
            .add_source(File::from_str(
                "mqtt_host = \"file.local\"\nmqtt_port = 1883\n",
                config::FileFormat::Toml,
            ))
            .add_source(env_source(Some(HashMap::from([(
                "ECU_TO_MQTT_MQTT_HOST".to_string(),
                "env.local".to_string(),
            )]))))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(config.mqtt_host, "env.local");
        assert_eq!(config.mqtt_port, 1883);
    }

    #[test]
    fn config_is_looked_up_under_the_project_name() {
        let paths: Vec<String> = candidate_paths()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert!(paths.contains(&"/etc/ecu-to-mqtt/settings.toml".to_string()));
        assert!(paths.contains(&"./settings.toml".to_string()));
        // System-wide files are read first so a local file wins.
        let system = paths
            .iter()
            .position(|p| p == "/etc/ecu-to-mqtt/settings.toml")
            .unwrap();
        let local = paths.iter().position(|p| p == "./settings.toml").unwrap();
        assert!(system < local);
    }

    #[test]
    fn default_topic_is_generic() {
        assert_eq!(AppConfig::default().mqtt_base_topic, "/ECU/");
    }

    #[test]
    fn test_default_config_validation() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_load_configuration_from_file() {
        let toml_content = r#"
            port_name = "/dev/ttyACM0"
            baud_rate = 115200
            mqtt_host = "mqtt.example.com"
            mqtt_port = 1883
            mqtt_base_topic = "/test/ecu/"
            refresh_rate_ms = 500
        "#;
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, toml_content).unwrap();
        let config = load_configuration(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(config.port_name, "/dev/ttyACM0");
        assert_eq!(config.baud_rate, 115200);
        assert_eq!(config.mqtt_host, "mqtt.example.com");
        assert_eq!(config.refresh_rate_ms, 500);
    }

    #[test]
    fn test_invalid_baud_rate() {
        let config = AppConfig {
            baud_rate: 12345,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_mqtt_qos() {
        let config = AppConfig {
            mqtt_qos: 5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_refresh_rate() {
        let config = AppConfig {
            refresh_rate_ms: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_missing_port_name() {
        let config = AppConfig {
            port_name: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_retry_delay_validation() {
        let config = AppConfig {
            max_retry_delay_ms: 500,
            initial_retry_delay_ms: 1000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_log_levels() {
        for level in ["trace", "debug", "info", "warn", "error"] {
            let config = AppConfig {
                log_level: level.to_string(),
                ..Default::default()
            };
            assert!(config.validate().is_ok());
        }
    }

    #[test]
    fn test_invalid_log_level() {
        let config = AppConfig {
            log_level: "verbose".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_tcp_connection_type_requires_host() {
        let config = AppConfig {
            connection_type: "tcp".to_string(),
            tcp_host: None,
            tcp_port: Some(4096),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_tcp_connection_type_requires_port() {
        let config = AppConfig {
            connection_type: "tcp".to_string(),
            tcp_host: Some("192.168.1.100".to_string()),
            tcp_port: None,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_tcp_connection_type_valid() {
        let config = AppConfig {
            connection_type: "tcp".to_string(),
            tcp_host: Some("192.168.1.100".to_string()),
            tcp_port: Some(4096),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn socketcan_connection_requires_an_interface() {
        let mut config = AppConfig {
            ecu_protocol: EcuProtocol::HaltechCanV2,
            connection_type: "can".to_string(),
            can_interface: None,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.can_interface = Some("  ".to_string());
        assert!(config.validate().is_err());
        config.can_interface = Some("can0".to_string());
        config.validate().unwrap();
        assert_eq!(config.connection_display(), "CAN can0");
    }

    #[test]
    fn socketcan_connection_rejects_a_serial_profile() {
        let config = AppConfig {
            ecu_protocol: EcuProtocol::Speeduino,
            connection_type: "can".to_string(),
            can_interface: Some("can0".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn can_profiles_reject_a_plain_serial_port() {
        let config = AppConfig {
            ecu_protocol: EcuProtocol::AemnetCan,
            connection_type: "serial".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn can_profiles_still_accept_the_tcp_gateway() {
        let config = AppConfig {
            ecu_protocol: EcuProtocol::AemnetCan,
            connection_type: "tcp".to_string(),
            tcp_host: Some("127.0.0.1".to_string()),
            tcp_port: Some(29536),
            ..Default::default()
        };
        config.validate().unwrap();
    }

    #[test]
    fn test_invalid_connection_type() {
        let config = AppConfig {
            connection_type: "usb".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_mqtt_disabled_skips_mqtt_validation() {
        let config = AppConfig {
            mqtt_enabled: false,
            mqtt_host: String::new(),
            mqtt_port: 0,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_connection_display_serial() {
        let config = AppConfig::default();
        assert!(config.connection_display().contains("/dev/ttyACM0"));
    }

    #[test]
    fn test_connection_display_tcp() {
        let config = AppConfig {
            connection_type: "tcp".to_string(),
            tcp_host: Some("192.168.1.50".to_string()),
            tcp_port: Some(4096),
            ..Default::default()
        };
        let display = config.connection_display();
        assert!(display.contains("192.168.1.50"));
        assert!(display.contains("4096"));
    }
}
