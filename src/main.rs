//! # ecu-to-mqtt
//!
//! Read-only ECU telemetry gateway: Speeduino and MegaSquirt over serial/TCP,
//! documented manufacturer CAN streams over SocketCAN or a CAN gateway, to
//! MQTT, an HTTP API and a web dashboard. Nothing is ever written to the ECU
//! except documented read requests.

#![cfg_attr(
    test,
    allow(clippy::field_reassign_with_default, clippy::type_complexity)
)]

mod archive;
mod config;
mod errors;
mod health;
mod hub;
mod output;
mod protocol;
mod sample;
mod shared;
mod source;
mod tui;

use anyhow::Result;
use clap::Parser;
use config::{AppConfig, ConnectionKind, load_configuration};
use protocol::EcuProtocol;
use shared::{Info, Shared};
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Read-only ECU telemetry gateway to MQTT, HTTP and a web dashboard.
#[derive(Debug, Parser)]
#[command(version, about, after_help = profiles_help())]
struct Cli {
    /// Path to a TOML config file (default: standard locations, then ./settings.toml)
    #[arg(short, long, value_name = "FILE")]
    config: Option<String>,

    /// Validate the configuration, print the effective settings and exit
    #[arg(long)]
    check_config: bool,

    /// List serial ports ranked by ECU likelihood and exit
    #[arg(long)]
    list_ports: bool,

    /// With --list-ports: ask each port for its ECU signature (sends only `Q`)
    #[arg(long, requires = "list_ports")]
    probe: bool,

    /// Run the built-in Speeduino simulator instead of a real ECU
    #[arg(long)]
    simulate: bool,

    /// Never start the terminal dashboard, even on a terminal
    #[arg(long)]
    headless: bool,

    /// Query the local HTTP /health endpoint and exit 0 (healthy) or 1
    #[arg(long)]
    healthcheck: bool,
}

fn profiles_help() -> String {
    let mut s = String::from("ECU profiles (ecu_protocol):\n");
    s.push_str(&format!("  {:<24} {}\n", "auto", EcuProtocol::Auto.label()));
    for p in EcuProtocol::ALL {
        s.push_str(&format!("  {:<24} {}\n", p.name(), p.label()));
    }
    s.push_str(
        "\nEvery setting can be overridden with ECU_TO_MQTT_<KEY>, e.g. ECU_TO_MQTT_MQTT_HOST.",
    );
    s
}

fn init_logging(config: &AppConfig, writer: Option<tui::TuiWriter>) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));
    match writer {
        Some(w) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(w)
            .init(),
        None if config.log_json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init(),
        None => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .with_ansi(std::io::stdout().is_terminal())
            .init(),
    }
}

async fn list_ports(config: &AppConfig, probe: bool) {
    let list = source::link::candidates();
    if list.is_empty() {
        println!("No serial ports found.");
        return;
    }
    for c in list {
        print!("{:<32} score {:>3}  {}", c.path, c.score, c.description);
        if probe {
            match source::link::Link::open_serial(&c.path, config.baud_rate) {
                Ok(mut link) => {
                    tokio::time::sleep(Duration::from_millis(2000)).await;
                    match source::poller::identify(&mut link, 2).await {
                        Some(sig) => {
                            let p = EcuProtocol::from_signature(&sig)
                                .map(|p| p.name())
                                .unwrap_or("unknown");
                            print!("  → {sig:?} ({p})");
                        }
                        None => print!("  → no reply"),
                    }
                }
                Err(e) => print!("  → {e:#}"),
            }
        }
        println!();
    }
}

/// `--healthcheck`: minimal HTTP/1.0 GET so container images need no curl.
fn healthcheck(config: &AppConfig) -> Result<bool> {
    use std::io::{Read, Write};
    anyhow::ensure!(
        config.http_enabled,
        "http_enabled is false; nothing to probe"
    );
    let host = match config.http_bind.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        h => h,
    };
    let addr: std::net::SocketAddr = format!("{host}:{}", config.http_port).parse()?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut response = String::new();
    stream.take(4096).read_to_string(&mut response)?;
    println!("{}", response.lines().last().unwrap_or_default());
    Ok(response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = match load_configuration(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e:#}");
            std::process::exit(2);
        }
    };
    if cli.simulate {
        config.connection_type = "simulator".into();
        config.validate()?;
    }

    if cli.list_ports {
        list_ports(&config, cli.probe).await;
        return Ok(());
    }
    if cli.healthcheck {
        let healthy = healthcheck(&config).unwrap_or_else(|e| {
            eprintln!("healthcheck failed: {e:#}");
            false
        });
        std::process::exit(if healthy { 0 } else { 1 });
    }
    if cli.check_config {
        println!("{}", serde_json::to_string_pretty(&config.redacted())?);
        println!(
            "\nConfiguration OK ({})",
            config
                .config_path
                .as_deref()
                .unwrap_or("defaults + environment")
        );
        return Ok(());
    }

    let no_tui_env = std::env::var("ECU_TO_MQTT_NO_TUI").is_ok_and(|v| v == "1");
    let interactive = !cli.headless && !no_tui_env && std::io::stdout().is_terminal();
    let log_buffer = Arc::new(Mutex::new(VecDeque::new()));
    init_logging(
        &config,
        interactive.then(|| tui::TuiWriter::new(log_buffer.clone())),
    );

    let config = Arc::new(config);
    info!("ecu-to-mqtt v{} starting", env!("CARGO_PKG_VERSION"));
    match &config.config_path {
        Some(p) => info!("Configuration: {p}"),
        None => warn!("No configuration file found; using defaults and environment"),
    }
    let initial_protocol = match config.connection_kind() {
        ConnectionKind::Simulator => EcuProtocol::Speeduino,
        _ => config.ecu_protocol,
    };
    let outputs = output::describe(&config);
    let shared = Shared::new(
        Info {
            version: env!("CARGO_PKG_VERSION"),
            profile: config.ecu_protocol.name().into(),
            connection: config.connection_display(),
            mqtt: config
                .mqtt_enabled
                .then(|| format!("{}:{}", config.mqtt_host, config.mqtt_port)),
            outputs: outputs.clone(),
        },
        initial_protocol,
        config.mqtt_enabled,
    );
    info!(
        "Profile: {} · connection: {}",
        config.ecu_protocol.label(),
        config.connection_display()
    );
    for o in &outputs {
        info!("Output: {o}");
    }
    if config.mqtt_enabled {
        info!(
            "MQTT base topic: {} ({})",
            config.base_topic(),
            config.base_topic_origin()
        );
    }

    let cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();
    let archive = config
        .telemetry_log
        .clone()
        .map(|path| archive::spawn(path, shared.clone()));
    let (source_tx, source_rx) = mpsc::channel(256);
    tasks.spawn(hub::run(source_rx, shared.clone(), archive, cancel.clone()));
    output::spawn_all(&config, &shared, &cancel, &mut tasks);
    source::spawn(&config, source_tx, cancel.clone());

    {
        let c = cancel.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            c.cancel();
        });
    }

    if interactive {
        if let Err(e) = tui::run_tui(shared.clone(), log_buffer, cancel.clone()).await {
            eprintln!("TUI error: {e:#}");
        }
        cancel.cancel();
    } else {
        cancel.cancelled().await;
    }

    info!("Shutting down");
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    info!("Shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => info!("Received Ctrl+C"),
            _ = term.recv() => info!("Received SIGTERM"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received Ctrl+C");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
        let cli = Cli::parse_from(["ecu-to-mqtt", "-c", "x.toml", "--simulate", "--headless"]);
        assert_eq!(cli.config.as_deref(), Some("x.toml"));
        assert!(cli.simulate && cli.headless);
        assert!(Cli::try_parse_from(["ecu-to-mqtt", "--probe"]).is_err());
        assert!(profiles_help().contains("haltech_can_v2"));
    }
}
