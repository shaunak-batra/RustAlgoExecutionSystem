//! # Execution Engine
//!
//! Core execution engine for algorithmic trading. This crate provides:
//!
//! - **Event Loop**: Main execution engine that processes orders and manages state
//! - **State Management**: Position tracking, order management, and execution state
//! - **Risk Management**: Pre-trade and post-trade risk checks
//! - **Smart Order Routing**: Multi-venue routing with configurable strategies
//!
//! ## Key Components
//!
//! - [`ExecutionEngine`]: Main event loop that handles order submission and execution
//! - [`EngineState`]: Central state store for positions, orders, and execution history
//! - [`RiskChecker`]: Pre-trade and post-trade risk validation
//! - [`SmartRouter`]: Intelligent order routing across multiple venues
//!
//! ## Example Usage
//!
//! ```no_run
//! use engine::{ExecutionEngine, RiskConfig};
//! use tokio::sync::mpsc;
//!
//! # async fn example() {
//! let (tx, rx) = mpsc::channel(100);
//! let engine = ExecutionEngine::new("BTC-USD".to_string(), 100)
//!     .with_risk_config(RiskConfig::default());
//! engine.run(rx).await;
//! # }
//! ```

pub mod state;
pub mod risk;
pub mod event_loop;
pub mod routing;

pub use api::{EngineCommand, ParentOrder, ParentOrderStatus, Position};
pub use event_loop::ExecutionEngine;
pub use risk::{RiskChecker, RiskConfig, RiskError};
pub use state::EngineState;
pub use routing::*;
