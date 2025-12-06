//! # Algorithmic Trading Strategies
//!
//! Core library of algorithmic trading strategies for optimal order execution.
//!
//! ## Available Strategies
//!
//! - **TWAP (Time-Weighted Average Price)**: Splits orders evenly over time
//! - **Adaptive TWAP**: TWAP with dynamic adjustment based on market conditions
//! - **VWAP (Volume-Weighted Average Price)**: Matches historical volume patterns
//! - **POV (Percentage of Volume)**: Executes as a percentage of market volume
//! - **Implementation Shortfall (IS)**: Minimizes cost relative to decision price
//!
//! ## Key Features
//!
//! - **Pure Functions**: All algorithms are deterministic and side-effect free
//! - **FFI-Safe**: Compatible with C/C++ and other language bindings
//! - **Validated Input**: Comprehensive error handling for invalid parameters
//! - **Simulation Ready**: Designed for backtesting and live trading
//!
//! ## Example Usage
//!
//! ```
//! use algo_core::twap::{TwapParams, compute_twap_schedule};
//!
//! let params = TwapParams {
//!     start_ns: 0,
//!     end_ns: 10_000_000_000, // 10 seconds
//!     total_qty: 1000,
//!     num_slices: 10,
//! };
//!
//! let schedule = compute_twap_schedule(params);
//! assert_eq!(schedule.len(), 10);
//! ```

pub mod twap;
pub mod pov;
pub mod vwap;
pub mod is;
pub mod adaptive_twap;

pub use twap::*;
pub use pov::*;
pub use vwap::*;
pub use is::*;
pub use adaptive_twap::*;
