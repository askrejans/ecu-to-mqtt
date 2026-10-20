//! Configuration management module
//!
//! Handles loading, validation, and environment variable overrides for application configuration.
//! Supports `.env` files via dotenvy, TOML config files, and `ECU_TO_MQTT_*` env var overrides.

use anyhow::{Context, Result, bail};
use config::{Config, Environment, File};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// Valid baud rates for serial communication
const VALID_BAUD_RATES: &[u32] = &[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

/// Prefix for environment variable overrides, e.g. `ECU_TO_MQTT_MQTT_HOST`.
pub const ENV_PREFIX: &str = "ECU_TO_MQTT";

/// Resolved `connection_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Serial,
    Tcp,
    Can,
    Simulator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MqttFormat {
    /// `<base>/telemetry` canonical envelope (G86 app contract)
    Canonical,
    /// Speeduino scalar topics (`<base>/RPM`, `<base>/CLT` …), change-detected
    Legacy,
    /// `<base>/state` complete JSON document (channels, Speeduino detail, status flags)
    Json,
    /// Home Assistant MQTT discovery for the main channels
    Homeassistant,
}

fn default_mqtt_formats() -> Vec<MqttFormat> {
    vec![MqttFormat::Canonical, MqttFormat::Legacy]
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

fn list_formats<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<MqttFormat>, D::Error> {
    let items = match StringOrList::deserialize(d)? {
        StringOrList::One(s) => s
            .split([',', ';', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        StringOrList::Many(v) => v,
    };
    items
        .into_iter()
        .map(|s| {
            MqttFormat::deserialize(serde::de::value::StringDeserializer::<D::Error>::new(
                s.to_lowercase(),
            ))
        })
        .collect()
}

/// Main application configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Optional absolute path for synced canonical NDJSON capture.
    #[serde(default)]
    pub telemetry_log: Option<String>,
    /// Read-only ECU protocol. See ECU_PROTOCOLS.md.
    #[serde(default)]
    pub ecu_protocol: crate::protocol::EcuProtocol,
    /// Override the profile CAN base identifier; frame format is profile-specific.
    #[serde(default)]
    pub can_base_id: Option<u32>,
    // --- Connection type ---
    /// Connection type: "serial" (hardware UART), "tcp" (raw TCP socket / WiFi
    /// bridge / CAN gateway), "can" (native SocketCAN interface, Linux only) or
    /// "simulator" (built-in Speeduino simulator)
    #[serde(default = "default_connection_type")]
    pub connection_type: String,

    /// SocketCAN interface for `connection_type = "can"`, e.g. "can0" or "vcan0".
    pub can_interface: Option<String>,

    /// TCP host for "tcp" connection_type (e.g. "192.168.1.100" or WiFi bridge hostname)
    pub tcp_host: Option<String>,

    /// TCP port for "tcp" connection_type (e.g. 4096 for most serial-to-TCP bridges)
    pub tcp_port: Option<u16>,

    // --- Serial port configuration (used when connection_type = "serial") ---
    /// Serial device path (e.g. "/dev/ttyACM0", "COM3"), or "auto" to find the
    /// ECU by its signature on the available serial ports
    #[serde(default = "default_port_name")]
    pub port_name: String,

    /// Baud rate for serial communication with Speeduino ECU
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,

    /// Expected data packet length from ECU in bytes.
    /// Only the first request uses it; the real length is learned from the reply
    /// (firmware 2025.01 sends 130 bytes).
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

    /// Read the MQTT password from a file (Docker/systemd secrets).
    pub mqtt_password_file: Option<String>,

    /// What to publish: canonical, legacy, json, homeassistant.
    #[serde(default = "default_mqtt_formats", deserialize_with = "list_formats")]
    pub mqtt_formats: Vec<MqttFormat>,

    /// Cap for legacy scalar and JSON state topics (Hz); 0 = every sample.
    /// The canonical envelope is never throttled.
    #[serde(default = "default_mqtt_max_rate_hz")]
    pub mqtt_max_rate_hz: f64,

    /// Home Assistant discovery prefix.
    #[serde(default = "default_ha_prefix")]
    pub mqtt_homeassistant_prefix: String,

    // --- HTTP API / web dashboard ---
    #[serde(default)]
    pub http_enabled: bool,
    #[serde(default = "default_http_bind")]
    pub http_bind: String,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    /// Bearer token required for `/api/*` (health and metrics stay open).
    pub http_token: Option<String>,
    /// `Access-Control-Allow-Origin` for browser dashboards (unset = no CORS).
    pub http_cors_origin: Option<String>,
    /// Serve the built-in web dashboard at `/`.
    #[serde(default = "default_true")]
    pub http_ui: bool,

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
fn default_true() -> bool {
    true
}
fn default_mqtt_max_rate_hz() -> f64 {
    10.0
}
fn default_ha_prefix() -> String {
    "homeassistant".to_string()
}
fn default_http_bind() -> String {
    "127.0.0.1".to_string()
}
fn default_http_port() -> u16 {
    9091
}
fn default_connection_type() -> String {
    "serial".to_string()
}
fn default_port_name() -> String {
    "auto".to_string()
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
        // Every field has a serde default, so an empty document is a valid config.
        serde_json::from_str("{}").expect("defaults deserialize")
    }
}

impl AppConfig {
    pub fn connection_kind(&self) -> ConnectionKind {
        match self.connection_type.to_lowercase().as_str() {
            "tcp" => ConnectionKind::Tcp,
            "can" => ConnectionKind::Can,
            "simulator" => ConnectionKind::Simulator,
            _ => ConnectionKind::Serial,
        }
    }

    pub fn wants(&self, format: MqttFormat) -> bool {
        self.mqtt_formats.contains(&format)
    }

    /// Settings as JSON with secrets masked (`--check-config`, `/api/v1/config`).
    pub fn redacted(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        for key in ["mqtt_password", "http_token"] {
            if v.get(key).is_some_and(|x| !x.is_null()) {
                v[key] = "***".into();
            }
        }
        v
    }

    /// Password from `mqtt_password_file` (trimmed) or `mqtt_password`.
    pub fn mqtt_password(&self) -> Result<Option<String>> {
        match &self.mqtt_password_file {
            Some(path) => Ok(Some(
                std::fs::read_to_string(path)
                    .with_context(|| format!("reading mqtt_password_file {path}"))?
                    .trim_end_matches(['\r', '\n'])
                    .to_string(),
            )),
            None => Ok(self.mqtt_password.clone()),
        }
    }

    /// Validate all configuration values against acceptable ranges and constraints.
    pub fn validate(&self) -> Result<()> {
        if self
            .telemetry_log
            .as_ref()
            .is_some_and(|p| !Path::new(p).is_absolute())
        {
            bail!("telemetry_log must be an absolute path");
        }
        let connection = self.connection_type.to_lowercase();
        let protocol = self.ecu_protocol;
        if protocol.is_can()
            && connection != "can"
            && connection != "tcp"
            && connection != "simulator"
        {
            bail!(
                "CAN profiles require connection_type = \"can\" (SocketCAN) or \"tcp\" (CAN gateway)"
            );
        }
        if !protocol.is_can() && connection == "can" {
            bail!(
                "connection_type = \"can\" needs a CAN profile; {} is a serial protocol",
                protocol.name()
            );
        }
        let limit = if protocol.extended() {
            0x1fff_ffff
        } else {
            0x7ff
        };
        if self.can_base_id.is_some_and(|id| {
            id.checked_add(protocol.last_can_offset())
                .is_none_or(|last| last > limit)
        }) {
            bail!("CAN base identifier leaves no room for broadcast groups");
        }
        match connection.as_str() {
            "can" => {
                if self
                    .can_interface
                    .as_deref()
                    .is_none_or(|n| n.trim().is_empty())
                {
                    bail!("can_interface is required when connection_type = \"can\"");
                }
            }
            "serial" => {
                if self.port_name.is_empty() {
                    bail!("port_name must not be empty (use \"auto\" to detect)");
                }
                if !VALID_BAUD_RATES.contains(&self.baud_rate) {
                    bail!("baud_rate must be one of {VALID_BAUD_RATES:?}");
                }
            }
            "tcp" => {
                if self.tcp_host.as_deref().is_none_or(str::is_empty) {
                    bail!("tcp_host is required when connection_type = \"tcp\"");
                }
                if self.tcp_port.is_none() {
                    bail!("tcp_port is required when connection_type = \"tcp\"");
                }
            }
            "simulator" => {}
            other => bail!(
                "connection_type must be \"serial\", \"tcp\", \"can\" or \"simulator\", got \"{other}\""
            ),
        }
        if !(119..=256).contains(&self.expected_data_length) {
            bail!("expected_data_length must be between 119 and 256");
        }
        if self.read_timeout_ms == 0 || self.read_timeout_ms > 30000 {
            bail!("read_timeout_ms must be between 1 and 30000");
        }
        if self.mqtt_enabled {
            if self.mqtt_host.is_empty() {
                bail!("mqtt_host must not be empty");
            }
            if self.mqtt_port == 0 {
                bail!("mqtt_port must be greater than 0");
            }
            if self.mqtt_base_topic.trim_matches('/').is_empty()
                || self.mqtt_base_topic.contains(['#', '+'])
            {
                bail!("mqtt_base_topic must be a non-empty topic without wildcards");
            }
            if !(0..=2).contains(&self.mqtt_qos) {
                bail!("mqtt_qos must be 0, 1 or 2");
            }
            if let Some(ca) = &self.mqtt_ca_cert_path
                && !Path::new(ca).exists()
            {
                bail!("mqtt_ca_cert_path does not exist: {ca}");
            }
            if self.mqtt_client_cert_path.is_some() != self.mqtt_client_key_path.is_some() {
                bail!("mqtt_client_cert_path and mqtt_client_key_path must be set together");
            }
            if self.message_buffer_size == 0 {
                bail!("message_buffer_size must be greater than 0");
            }
            if self.mqtt_formats.is_empty() {
                bail!(
                    "mqtt_formats must list at least one of canonical, legacy, json, homeassistant"
                );
            }
            if !(self.mqtt_max_rate_hz.is_finite() && self.mqtt_max_rate_hz >= 0.0) {
                bail!("mqtt_max_rate_hz must be >= 0");
            }
        }
        if self.http_enabled {
            if self.http_port == 0 {
                bail!("http_port must be greater than 0");
            }
            self.http_bind
                .parse::<std::net::IpAddr>()
                .context("http_bind must be an IP address such as 127.0.0.1 or 0.0.0.0")?;
        }
        if self.refresh_rate_ms == 0 || self.refresh_rate_ms > 10000 {
            bail!("refresh_rate_ms must be between 1 and 10000");
        }
        if self.max_retry_count == 0 {
            bail!("max_retry_count must be greater than 0");
        }
        if self.initial_retry_delay_ms == 0 {
            bail!("initial_retry_delay_ms must be greater than 0");
        }
        if self.max_retry_delay_ms < self.initial_retry_delay_ms {
            bail!("max_retry_delay_ms must be >= initial_retry_delay_ms");
        }
        let levels = ["trace", "debug", "info", "warn", "error"];
        if !levels.contains(&self.log_level.to_lowercase().as_str()) {
            bail!("log_level must be one of {levels:?}");
        }
        Ok(())
    }

    /// Returns a human-readable description of the ECU connection endpoint.
    pub fn connection_display(&self) -> String {
        match self.connection_type.to_lowercase().as_str() {
            "can" => format!("CAN {}", self.can_interface.as_deref().unwrap_or("?")),
            "simulator" => "simulator".to_string(),
            "tcp" => format!(
                "TCP {}:{}",
                self.tcp_host.as_deref().unwrap_or("?"),
                self.tcp_port.unwrap_or(0)
            ),
            _ => format!("{} @ {} baud", self.port_name, self.baud_rate),
        }
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

        found_path
    };

    builder = builder.add_source(env_source(None));

    let mut app_config: AppConfig = builder
        .build()
        .context("failed to read configuration")?
        .try_deserialize()
        .context("invalid configuration")?;
    app_config.config_path = loaded_from;
    app_config.validate()?;
    Ok(app_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol::EcuProtocol;
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
        assert_eq!(config.connection_display(), "auto @ 115200 baud");
        let config = AppConfig {
            port_name: "/dev/ttyACM0".into(),
            ..AppConfig::default()
        };
        assert!(config.connection_display().contains("/dev/ttyACM0"));
    }

    #[test]
    fn plug_and_play_defaults() {
        let c = AppConfig::default();
        assert_eq!(c.ecu_protocol, EcuProtocol::Auto);
        assert_eq!(c.port_name, "auto");
        assert_eq!(c.mqtt_formats, [MqttFormat::Canonical, MqttFormat::Legacy]);
        assert_eq!(c.http_bind, "127.0.0.1");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn new_keys_from_environment() {
        let config: AppConfig = Config::builder()
            .add_source(env_source(Some(HashMap::from([
                (
                    "ECU_TO_MQTT_MQTT_FORMATS".to_string(),
                    "canonical, json,homeassistant".to_string(),
                ),
                ("ECU_TO_MQTT_HTTP_ENABLED".to_string(), "true".to_string()),
                ("ECU_TO_MQTT_HTTP_TOKEN".to_string(), "12345".to_string()),
                (
                    "ECU_TO_MQTT_CONNECTION_TYPE".to_string(),
                    "simulator".to_string(),
                ),
            ]))))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(
            config.mqtt_formats,
            [
                MqttFormat::Canonical,
                MqttFormat::Json,
                MqttFormat::Homeassistant
            ]
        );
        assert!(config.http_enabled);
        assert_eq!(config.http_token.as_deref(), Some("12345"));
        assert_eq!(config.connection_kind(), ConnectionKind::Simulator);
        config.validate().unwrap();
        assert_eq!(config.redacted()["http_token"], "***");
    }

    #[test]
    fn new_keys_are_validated() {
        for mutate in [
            (|c: &mut AppConfig| c.mqtt_formats.clear()) as fn(&mut AppConfig),
            |c| c.mqtt_base_topic = "a/#".into(),
            |c| c.mqtt_max_rate_hz = -1.0,
            |c| c.mqtt_client_cert_path = Some("c.pem".into()),
            |c| {
                c.http_enabled = true;
                c.http_bind = "localhost".into()
            },
            |c| c.connection_type = "usb".into(),
        ] {
            let mut c = AppConfig::default();
            mutate(&mut c);
            assert!(c.validate().is_err());
        }
    }

    #[test]
    fn password_file_wins_and_is_trimmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pw");
        fs::write(&path, "s3cret\n").unwrap();
        let c = AppConfig {
            mqtt_password: Some("ignored".into()),
            mqtt_password_file: Some(path.to_string_lossy().into_owned()),
            ..AppConfig::default()
        };
        assert_eq!(c.mqtt_password().unwrap().as_deref(), Some("s3cret"));
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
