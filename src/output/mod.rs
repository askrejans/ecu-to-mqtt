//! Outputs. Each subscribes to the bus independently; a slow or failing output
//! never affects acquisition or the other outputs.

pub mod http;
pub mod mqtt;
pub mod web;

use crate::config::AppConfig;
use crate::shared::Shared;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::error;

pub fn describe(config: &AppConfig) -> Vec<String> {
    let mut out = Vec::new();
    if config.mqtt_enabled {
        let formats: Vec<String> = config
            .mqtt_formats
            .iter()
            .map(|f| format!("{f:?}").to_lowercase())
            .collect();
        out.push(format!(
            "mqtt://{}:{} [{}]",
            config.mqtt_host,
            config.mqtt_port,
            formats.join(",")
        ));
    }
    if config.http_enabled {
        out.push(format!("http://{}:{}", config.http_bind, config.http_port));
    }
    if let Some(p) = &config.telemetry_log {
        out.push(format!("archive {p}"));
    }
    out
}

pub fn spawn_all(
    config: &Arc<AppConfig>,
    shared: &Arc<Shared>,
    cancel: &CancellationToken,
    set: &mut JoinSet<()>,
) {
    fn guard(name: &'static str, r: anyhow::Result<()>) {
        if let Err(e) = r {
            error!("{name} output stopped: {e:#}");
        }
    }
    if config.mqtt_enabled {
        let (c, s, k) = (config.clone(), shared.clone(), cancel.clone());
        set.spawn(async move { guard("MQTT", mqtt::run(c, s, k).await) });
    }
    if config.http_enabled {
        let (c, s, k) = (config.clone(), shared.clone(), cancel.clone());
        set.spawn(async move { guard("HTTP", http::run(c, s, k).await) });
    }
}
