use api::start_grpc_server;
use engine::{EngineCommand, ExecutionEngine};
use tokio::sync::mpsc;
use tracing::{info, Level};
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("Starting Algo Execution Engine...");

    let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>(1000);

    let engine = ExecutionEngine::new("BTC-USD".to_string(), 100);

    let engine_handle = tokio::spawn(async move {
        engine.run(cmd_rx).await;
    });

    let grpc_addr = "0.0.0.0:9090";
    info!("Starting gRPC server on {}...", grpc_addr);

    let cmd_tx_clone = cmd_tx.clone();
    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = start_grpc_server(cmd_tx_clone, grpc_addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    info!("Engine and gRPC server running. Press Ctrl+C to stop...");

    tokio::signal::ctrl_c().await?;

    info!("Shutdown signal received, stopping engine...");

    cmd_tx.send(EngineCommand::Shutdown).await?;

    engine_handle.await?;
    grpc_handle.abort();

    info!("Engine stopped successfully");

    Ok(())
}
