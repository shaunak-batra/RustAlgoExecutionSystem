//! # gRPC API
//!
//! gRPC server implementation for the execution engine.
//!
//! ## Services
//!
//! - **ExecutionService**: Submit and manage parent orders, query order status
//! - **Position Management**: Query positions and P&L
//! - **Fill Streaming**: Real-time fill event notifications
//!
//! ## Key Components
//!
//! - [`ExecutionServiceImpl`]: gRPC service implementation
//! - [`start_grpc_server`]: Server initialization and startup
//! - [`ParentOrder`]: Parent order representation with child order tracking
//! - [`Position`]: Position tracking with realized and unrealized P&L
//!
//! ## Example Usage
//!
//! ```no_run
//! use api::start_grpc_server;
//! use tokio::sync::mpsc;
//!
//! # async fn example() {
//! let (tx, _rx) = mpsc::channel(100);
//! start_grpc_server(tx, "127.0.0.1:50051").await.unwrap();
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
