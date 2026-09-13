//! Runs the engine task and the gRPC API together, and shuts both down in an
//! order that cannot hang.

use crate::event_loop::ExecutionEngine;
use crate::executor::EngineCore;
use api::EngineCommand;
use std::future::Future;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Sizing and timing for [`serve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    /// How often due child orders are released and quotes refreshed.
    pub tick_interval: Duration,
    /// Capacity of the command channel from the gRPC service to the engine.
    pub command_buffer: usize,
    /// Roughly how far a fill subscriber may fall behind; see [`ExecutionEngine::new`].
    pub fill_buffer: usize,
}

/// Serves the execution API on `listener` until `shutdown` completes, then stops
/// the engine task and waits for it.
///
/// The order matters. Once `shutdown` completes the server stops accepting work
/// and waits for every open stream to finish, and a `StreamFills` stream only
/// finishes when the engine drops its fill sender. So the engine is told to stop
/// as part of the shutdown signal, before the server drains. Stopping it after
/// the server returned would wait forever while any client stayed subscribed.
pub async fn serve(
    core: EngineCore,
    options: ServiceOptions,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), tonic::transport::Error> {
    let (commands, receiver) = mpsc::channel(options.command_buffer.max(1));
    let engine = ExecutionEngine::new(core, options.tick_interval, options.fill_buffer);
    let engine_task = tokio::spawn(engine.run(receiver));

    let stop_engine = commands.clone();
    let shutdown = async move {
        shutdown.await;
        let _ = stop_engine.send(EngineCommand::Shutdown).await;
    };
    let served = api::serve_with_listener(commands.clone(), listener, shutdown).await;

    // If the server failed on its own, the engine is still running.
    let _ = commands.send(EngineCommand::Shutdown).await;
    let _ = engine_task.await;
    served
}
