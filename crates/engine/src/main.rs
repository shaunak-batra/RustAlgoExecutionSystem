//! Engine server: loads settings, then runs the engine task and the gRPC API
//! until Ctrl+C.
//!
//! Usage: `engine [--config <path>]` (default: `config/default.toml`, relative
//! to the working directory).

use engine::{EngineConfig, EngineCore, ServiceOptions};
use std::process::ExitCode;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{error, info};

const DEFAULT_CONFIG: &str = "config/default.toml";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config_path = config_path(std::env::args().skip(1))?;
    let settings = config::Settings::load(&config_path).map_err(|e| e.to_string())?;
    let engine_config = EngineConfig::from_settings(&settings)
        .map_err(|e| format!("invalid settings in {config_path}: {e}"))?;
    let core = EngineCore::new(engine_config).map_err(|e| e.to_string())?;
    let listener = TcpListener::bind(settings.api.listen_addr)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", settings.api.listen_addr))?;

    info!("serving the execution API on {}", settings.api.listen_addr);
    let options = ServiceOptions {
        tick_interval: Duration::from_millis(settings.engine.tick_interval_ms),
        command_buffer: settings.engine.command_buffer,
        fill_buffer: settings.engine.fill_buffer,
    };
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutting down");
    };
    engine::serve(core, options, listener, shutdown)
        .await
        .map_err(|e| format!("the gRPC server failed: {e}"))
}

fn config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next(), args.next(), args.next()) {
        (None, _, _) => Ok(DEFAULT_CONFIG.to_string()),
        (Some(flag), Some(path), None) if flag == "--config" => Ok(path),
        _ => Err("usage: engine [--config <path>]".to_string()),
    }
}
