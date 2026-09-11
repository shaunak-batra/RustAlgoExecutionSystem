//! Engine server: loads settings, then runs the engine task and the gRPC API
//! until Ctrl+C.
//!
//! Usage: `engine [--config <path>]` (default: `config/default.toml`).

use engine::{EngineConfig, EngineCore, ExecutionEngine};
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::mpsc;
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

    let (commands, receiver) = mpsc::channel(settings.engine.command_buffer);
    let engine = ExecutionEngine::new(
        core,
        Duration::from_millis(settings.engine.tick_interval_ms),
        settings.engine.fill_buffer,
    );
    let engine_task = tokio::spawn(engine.run(receiver));

    info!("serving the execution API on {}", settings.api.listen_addr);
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutting down");
    };
    let served = api::serve(commands.clone(), settings.api.listen_addr, shutdown).await;

    let _ = commands.send(api::EngineCommand::Shutdown).await;
    let _ = engine_task.await;
    served.map_err(|e| format!("the gRPC server failed: {e}"))
}

fn config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next(), args.next(), args.next()) {
        (None, _, _) => Ok(DEFAULT_CONFIG.to_string()),
        (Some(flag), Some(path), None) if flag == "--config" => Ok(path),
        _ => Err("usage: engine [--config <path>]".to_string()),
    }
}
