//! # Speeduino to MQTT
//!
//! Bridges a Speeduino ECU (serial or TCP) to an MQTT broker.
//!
//! **Interactive mode** (TTY detected): renders a live TUI and optionally
//! writes to MQTT when `mqtt_enabled = true`.
//!
//! **Service mode** (no TTY / running under systemd): structured logging to
//! stdout, same ECU polling logic.

mod can_profiles;
mod can_stream;
mod config;
mod connection;
mod ecu_data_parser;
mod ecu_protocol;
mod ecu_serial_comms_handler;
mod errors;
mod mqtt_handler;
mod telemetry_frame;
mod tui;

use crate::config::{AppConfig, load_configuration};
use crate::ecu_data_parser::{SpeeduinoData, process_speeduino_realtime_data};
use crate::ecu_serial_comms_handler::EcuSerialHandler;
use crate::mqtt_handler::{MqttHandler, MqttMessage};
use crate::tui::{TuiState, TuiWriter, run_tui};
use gumdrop::Options;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::select;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Options)]
struct CliOptions {
    #[options(help = "print help message")]
    help: bool,

    #[options(help = "path to config file (default: settings.toml)", meta = "FILE")]
    config: Option<String>,
}

fn print_help() {
    println!("Usage: speeduino-to-mqtt [options]");
    println!();
    println!("Options:");
    println!("  -h, --help               Print this help message");
    println!("  -c, --config FILE        Path to TOML config file");
    println!();
    println!("Environment variables (SPEEDUINO_ prefix overrides config file):");
    println!("  SPEEDUINO_ECU_PROTOCOL     ECU profile name (see ECU_PROTOCOLS.md)");
    println!("  SPEEDUINO_CAN_BASE_ID      CAN identifier override (decimal)");
    println!("  SPEEDUINO_CONNECTION_TYPE  'serial' (default) or 'tcp'");
    println!("  SPEEDUINO_PORT_NAME        Serial device path");
    println!("  SPEEDUINO_BAUD_RATE        Serial baud rate");
    println!("  SPEEDUINO_TCP_HOST         TCP host (when connection_type=tcp)");
    println!("  SPEEDUINO_TCP_PORT         TCP port (when connection_type=tcp)");
    println!("  SPEEDUINO_MQTT_ENABLED     true/false – set false for display-only");
    println!("  SPEEDUINO_MQTT_HOST        MQTT broker hostname");
    println!("  SPEEDUINO_MQTT_PORT        MQTT broker port");
    println!("  SPEEDUINO_LOG_LEVEL        trace|debug|info|warn|error");
    println!("  .env file is loaded automatically from the working directory");
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// Set up tracing.
///
/// In service mode: write to stdout (JSON or pretty format per config).
/// In TUI mode: write to a [`TuiWriter`] that feeds the on-screen log panel.
fn init_logging_service(config: &AppConfig) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    if config.log_json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    }
}

fn init_logging_tui(config: &AppConfig, writer: TuiWriter) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(writer)
        .init();
}

// ---------------------------------------------------------------------------
// Signal handler
// ---------------------------------------------------------------------------

fn spawn_signal_handler(cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use futures_util::stream::StreamExt;
            use signal_hook::consts::signal::{SIGINT, SIGTERM};
            use signal_hook_tokio::Signals;
            match Signals::new([SIGTERM, SIGINT]) {
                Ok(mut signals) => {
                    while let Some(signal) = signals.next().await {
                        match signal {
                            SIGTERM | SIGINT => {
                                info!("Received shutdown signal ({})", signal);
                                cancel.cancel();
                                return;
                            }
                            _ => warn!("Unhandled signal: {}", signal),
                        }
                    }
                }
                Err(e) => error!("Failed to register signal handlers: {}", e),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            info!("Received Ctrl+C");
            cancel.cancel();
        }
    })
}

// ---------------------------------------------------------------------------
// ECU loop
// ---------------------------------------------------------------------------

async fn ecu_communication_loop(
    config: Arc<AppConfig>,
    mqtt_sender: Option<mpsc::Sender<MqttMessage>>,
    tui_state: Arc<RwLock<TuiState>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    if config.ecu_protocol.is_can() {
        return can_stream::run(config, mqtt_sender, cancel).await;
    }
    let mut handler = EcuSerialHandler::new((*config).clone());

    // Initial connection with backoff – retries indefinitely, never exits.
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match handler.connect().await {
            Ok(_) => {
                info!("Connected to ECU: {}", config.connection_display());
                {
                    let mut s = tui_state.write().await;
                    s.ecu_connected = true;
                    s.connection_address = config.connection_display();
                }
                break;
            }
            Err(e) => {
                error!("Failed to connect to ECU: {}", e);
                if handler.get_retry_count() >= config.max_retry_count {
                    warn!(
                        "Max connection attempts reached – resetting counter and retrying indefinitely"
                    );
                    handler.reset_retry_count();
                }
                // Ignore reconnect error; backoff sleep already happened inside reconnect().
                let _ = handler.reconnect().await;
            }
        }
    }

    let mut poll = interval(Duration::from_millis(config.refresh_rate_ms));
    let mut consecutive_errors: u32 = 0;
    const MAX_ERRORS: u32 = 10;

    info!(
        "ECU polling started at {}ms interval",
        config.refresh_rate_ms
    );

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("ECU loop: shutdown requested");
                break;
            }
            _ = poll.tick() => {}
        }

        if !handler.check_device_exists() {
            warn!("ECU device not found, attempting reconnect…");
            handler.disconnect().await;
            tui_state.write().await.ecu_connected = false;

            if handler.reconnect().await.is_ok() {
                consecutive_errors = 0;
                tui_state.write().await.ecu_connected = true;
            } else {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_ERRORS {
                    warn!(
                        "Reconnection failed too many times – resetting counter and retrying indefinitely"
                    );
                    handler.reset_retry_count();
                    consecutive_errors = 0;
                }
            }
            continue;
        }

        match handler.read_engine_data().await {
            Ok(data) => {
                debug!("Read {} bytes from ECU", data.len());
                let sender_ref = mqtt_sender.as_ref();
                if config.ecu_protocol != ecu_protocol::EcuProtocol::Speeduino {
                    match ecu_protocol::decode_compat112(&data) {
                        Ok(channels) => {
                            telemetry_frame::publish(
                                channels,
                                config.ecu_protocol.source(),
                                false,
                                &config,
                                sender_ref,
                            )
                            .await?;
                            consecutive_errors = 0;
                            handler.reset_retry_count();
                        }
                        Err(error) => {
                            tracing::warn!(%error, "MegaSquirt data rejected");
                            consecutive_errors += 1;
                        }
                    }
                    continue;
                }
                match process_speeduino_realtime_data(&data, &config, sender_ref).await {
                    Ok(ecu_data) => {
                        consecutive_errors = 0;
                        handler.reset_retry_count();
                        update_tui_ecu_data(&tui_state, ecu_data, &mqtt_sender).await;
                    }
                    Err(e) => {
                        error!("Failed to process ECU data: {}", e);
                        consecutive_errors += 1;
                    }
                }
            }
            Err(e) => {
                error!("Failed to read from ECU: {}", e);
                consecutive_errors += 1;
                tui_state.write().await.ecu_connected = false;

                if consecutive_errors >= MAX_ERRORS {
                    error!("Too many read errors, reconnecting…");
                    handler.disconnect().await;
                    match handler.reconnect().await {
                        Ok(_) => {
                            consecutive_errors = 0;
                            tui_state.write().await.ecu_connected = true;
                        }
                        Err(e) => {
                            warn!(
                                "Reconnect failed after read errors: {} – resetting and retrying indefinitely",
                                e
                            );
                            handler.reset_retry_count();
                            consecutive_errors = 0;
                            tui_state.write().await.ecu_connected = false;
                        }
                    }
                }
            }
        }
    }

    handler.disconnect().await;
    Ok(())
}

async fn update_tui_ecu_data(
    state: &Arc<RwLock<TuiState>>,
    data: SpeeduinoData,
    mqtt_sender: &Option<mpsc::Sender<MqttMessage>>,
) {
    let mut s = state.write().await;
    s.ecu_data = Some(data);
    if mqtt_sender.is_some() {
        s.messages_published = s.messages_published.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Welcome banner (service mode only)
// ---------------------------------------------------------------------------

fn display_welcome() {
    println!("\x1b[1;32m");
    println!("\nWelcome to Speeduino to MQTT");
    println!("============================");
    println!("\x1b[1;31m");
    println!("       ______");
    println!("      //  ||\\ \\");
    println!(" ____//___||_\\ \\___");
    println!(" )  _          _    \\");
    println!(" |_/ \\________/ \\___|");
    println!("___\\_/________\\_/______");
    println!("\x1b[1;32m");
    println!("============================");
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("============================\n\x1b[0m");
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = CliOptions::parse_args_default_or_exit();
    if opts.help {
        print_help();
        std::process::exit(0);
    }

    // Load config (also loads .env file via dotenvy)
    let config = match load_configuration(opts.config.as_deref()) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Shared cancellation token
    let cancel = CancellationToken::new();

    // Detect whether stdin is a TTY – show TUI when running interactively.
    // SPEEDUINO_NO_TUI=1 forces service/log mode (set by default in Docker).
    let force_no_tui = std::env::var("SPEEDUINO_NO_TUI")
        .map(|v| v == "1")
        .unwrap_or(false);
    let is_tty = !force_no_tui
        && config.ecu_protocol == ecu_protocol::EcuProtocol::Speeduino
        && atty::is(atty::Stream::Stdout);

    // Shared state for TUI
    let log_buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let tui_state: Arc<RwLock<TuiState>> = Arc::new(RwLock::new(TuiState {
        mqtt_enabled: config.mqtt_enabled,
        connection_address: config.connection_display(),
        mqtt_address: if config.mqtt_enabled {
            format!("{}:{}", config.mqtt_host, config.mqtt_port)
        } else {
            String::new()
        },
        ..TuiState::default()
    }));

    // Init logging – in TUI mode write to the shared log buffer
    if is_tty {
        let writer = TuiWriter::new(Arc::clone(&log_buffer));
        init_logging_tui(&config, writer);
    } else {
        display_welcome();
        init_logging_service(&config);
    }

    info!(
        "Configuration loaded; connection={}",
        config.connection_display()
    );

    // Signal handler
    let signals_task = spawn_signal_handler(cancel.clone());

    // Optional MQTT setup — handler must stay on the main task (paho futures are !Send)
    let (mqtt_sender, mqtt_handler_opt): (Option<mpsc::Sender<MqttMessage>>, Option<MqttHandler>) =
        if config.mqtt_enabled {
            info!(
                "Setting up MQTT connection to {}:{}",
                config.mqtt_host, config.mqtt_port
            );
            match MqttHandler::new(config.clone()) {
                Ok(mut handler) => match handler.connect().await {
                    Ok(_) => {
                        info!("MQTT connected");
                        tui_state.write().await.mqtt_connected = true;
                        let sender = handler.get_sender();
                        (Some(sender), Some(handler))
                    }
                    Err(e) => {
                        error!("Failed to connect to MQTT broker: {}", e);
                        if !is_tty {
                            return Err(e.into());
                        }
                        warn!("Continuing without MQTT (display-only mode)");
                        (None, None)
                    }
                },
                Err(e) => {
                    error!("Failed to create MQTT handler: {}", e);
                    return Err(e.into());
                }
            }
        } else {
            info!("MQTT disabled – ECU data will be displayed in TUI only");
            (None, None)
        };

    // ECU communication task
    let ecu_config = Arc::clone(&config);
    let ecu_state = Arc::clone(&tui_state);
    let ecu_cancel = cancel.clone();
    let ecu_task = tokio::spawn(async move {
        if let Err(e) = ecu_communication_loop(ecu_config, mqtt_sender, ecu_state, ecu_cancel).await
        {
            error!("ECU loop exited with error: {}", e);
        }
    });

    // TUI task (interactive mode only)
    let tui_task = if is_tty {
        let ts = Arc::clone(&tui_state);
        let lb = Arc::clone(&log_buffer);
        let tc = cancel.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = run_tui(ts, lb, tc).await {
                error!("TUI error: {}", e);
            }
        }))
    } else {
        None
    };

    info!("All tasks running. Press Ctrl+C to stop.");

    // Drive the MQTT publish task directly on the main task (paho futures are !Send).
    // ECU and TUI tasks are spawned because they only use Send types.
    select! {
        _ = cancel.cancelled() => {
            info!("Shutdown signal received");
        }
        _ = ecu_task => {
            warn!("ECU task terminated; shutting down");
            cancel.cancel();
        }
        result = async {
            if let Some(handler) = mqtt_handler_opt {
                handler.start_publishing_task().await
            } else {
                std::future::pending::<crate::errors::Result<()>>().await
            }
        } => {
            match result {
                Ok(_)  => info!("MQTT publish task completed"),
                Err(e) => error!("MQTT publish task error: {}", e),
            }
            tui_state.write().await.mqtt_connected = false;
            cancel.cancel();
        }
    }

    // Wait for TUI to finish restoring the terminal
    if let Some(t) = tui_task {
        let _ = t.await;
    }

    let _ = signals_task.await;

    info!("Goodbye!");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn megasquirt_profiles_read_tcp_reply_and_publish_canonical_mqtt() {
        use ecu_protocol::EcuProtocol;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
            time::timeout,
        };
        for profile in [
            EcuProtocol::Ms2,
            EcuProtocol::Ms3,
            EcuProtocol::Ms3Pro,
            EcuProtocol::Microsquirt,
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = Arc::new(AppConfig {
                ecu_protocol: profile,
                connection_type: "tcp".into(),
                tcp_host: Some("127.0.0.1".into()),
                tcp_port: Some(listener.local_addr().unwrap().port()),
                refresh_rate_ms: 1000,
                ..Default::default()
            });
            let cancel = CancellationToken::new();
            let (sender, mut receiver) = mpsc::channel(4);
            let worker = tokio::spawn(ecu_communication_loop(
                config,
                Some(sender),
                Arc::new(RwLock::new(TuiState::default())),
                cancel.clone(),
            ));
            let (mut ecu, _) = listener.accept().await.unwrap();
            let mut command = [0; 3];
            ecu.read_exact(&mut command).await.unwrap();
            assert_eq!(command, [b'a', 0, 6]);
            let mut reply = [0u8; 112];
            reply[6..8].copy_from_slice(&6000u16.to_be_bytes());
            reply[22..24].copy_from_slice(&1940u16.to_be_bytes());
            reply[26..28].copy_from_slice(&138u16.to_be_bytes());
            ecu.write_all(&reply[..37]).await.unwrap();
            // Longer than the former 150 ms initial wait + 5 ms drain gap.
            tokio::time::sleep(Duration::from_millis(180)).await;
            ecu.write_all(&reply[37..]).await.unwrap();
            let message = timeout(Duration::from_secs(2), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            let payload: serde_json::Value = serde_json::from_str(&message.payload).unwrap();
            assert_eq!(payload["source"], profile.source());
            assert_eq!(payload["channels"]["rpm"], 6000.);
            assert_eq!(payload["channels"]["coolantC"], 90.);
            assert_eq!(payload["channels"]["batteryV"], 13.8);
            assert_eq!(payload["partial"], false);
            cancel.cancel();
            timeout(Duration::from_secs(1), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn test_print_help_no_panic() {
        print_help();
    }

    #[test]
    fn test_display_welcome_no_panic() {
        display_welcome();
    }

    #[test]
    fn test_cli_options_help_flag() {
        let args = ["--help"];
        let opts = CliOptions::parse_args(&args, gumdrop::ParsingStyle::default());
        assert!(opts.is_ok());
        assert!(opts.unwrap().help);
    }
}
