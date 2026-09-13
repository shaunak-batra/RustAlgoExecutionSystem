//! # Execution Engine
//!
//! Works parent orders against a simulated venue.
//!
//! - [`EngineCore`]: all engine state and every state transition, driven by
//!   explicit timestamps and never by a clock, so a run is reproducible on a
//!   given build and platform and every scenario is unit-testable
//! - [`serve`]: runs the engine task and the gRPC API together, and shuts them
//!   down in an order that cannot hang
//! - [`ExecutionEngine`]: an async task that owns an `EngineCore`, serves
//!   [`EngineCommand`]s from a channel, ticks on a timer and broadcasts fills,
//!   with no shared mutable state or locks
//! - [`SimulatedVenue`]: an order book per symbol, quoted by a market maker
//!   whose quotes are refreshed every tick
//! - [`RiskLimits`]: pre-trade limits and the loss kill switch
//! - [`Position`]: exact integer position and P&L accounting
//!
//! ## Example
//!
//! ```
//! use api::{AlgorithmSpec, NewParentOrder, ParentOrderState};
//! use engine::{EngineConfig, EngineCore, RiskLimits, SymbolConfig};
//! use orderbook::{Price, Side};
//!
//! let mut engine = EngineCore::new(EngineConfig {
//!     risk: RiskLimits {
//!         max_order_qty: 1_000_000,
//!         max_order_notional: i128::MAX,
//!         max_position_qty: 1_000_000,
//!         max_gross_notional: i128::MAX,
//!         price_collar_bps: 10_000,
//!         max_loss: i128::MAX,
//!     },
//!     symbols: vec![SymbolConfig {
//!         symbol: "SIM".to_string(),
//!         reference_price: Price::from_f64(100.0),
//!         levels: 5,
//!         qty_per_level: 1_000,
//!         level_spacing_bps: 1,
//!     }],
//!     retained_finished_orders: 100,
//! })
//! .unwrap();
//!
//! const SECOND: u64 = 1_000_000_000;
//! let id = engine
//!     .submit(
//!         NewParentOrder {
//!             symbol: "SIM".to_string(),
//!             side: Side::Buy,
//!             quantity: 400,
//!             limit_price: None,
//!             start_ns: 0,
//!             end_ns: 4 * SECOND,
//!             algorithm: AlgorithmSpec::Twap { num_slices: 4 },
//!         },
//!         0,
//!     )
//!     .unwrap();
//!
//! // One child order per second, driven by explicit timestamps.
//! for second in 0..=4 {
//!     engine.tick(second * SECOND);
//! }
//! let status = engine.order_status(id).unwrap();
//! assert_eq!(status.state, ParentOrderState::Filled);
//! assert_eq!(status.children.len(), 4);
//! ```

pub mod accounting;
pub mod event_loop;
pub mod executor;
pub mod risk;
pub mod service;
pub mod state;
pub mod venue;

pub use accounting::{AccountingOverflow, Position, TickValue};
pub use api::EngineCommand;
pub use event_loop::ExecutionEngine;
pub use executor::{CancelError, EngineConfig, EngineCore, RejectReason};
pub use risk::{RiskLimits, RiskViolation, SymbolExposure};
pub use service::{serve, ServiceOptions};
pub use state::ParentOrder;
pub use venue::{SimulatedVenue, SymbolConfig, VenueError};
