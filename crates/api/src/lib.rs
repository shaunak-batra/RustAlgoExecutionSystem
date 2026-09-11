//! # gRPC API
//!
//! Transport layer for the execution engine.
//!
//! - [`proto`]: messages and service traits generated from `proto/execution.proto`
//! - [`EngineCommand`] and the view types: the channel protocol between the
//!   gRPC service and the engine task, which owns all engine state
//! - [`serve`] and [`serve_with_listener`]: run the service until a shutdown
//!   future completes
//!
//! ## Example
//!
//! ```no_run
//! use tokio::sync::mpsc;
//!
//! # async fn example() -> Result<(), tonic::transport::Error> {
//! // The receiving end goes to the engine task (see the `engine` crate).
//! let (commands, _engine_receiver) = mpsc::channel(1024);
//! let addr = "127.0.0.1:50051".parse().unwrap();
//! api::serve(commands, addr, async {
//!     let _ = tokio::signal::ctrl_c().await;
//! })
//! .await
//! # }
//! ```

// Generated from proto/execution.proto by build.rs (written to OUT_DIR).
pub mod proto {
    #![allow(clippy::all)]
    tonic::include_proto!("execution");
}

pub mod server;
pub mod types;

pub use server::*;
pub use types::*;
