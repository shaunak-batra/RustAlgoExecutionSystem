//! # Execution Algorithms
//!
//! Scheduling functions for splitting a parent order into child orders.
//!
//! - **TWAP (Time-Weighted Average Price)**: Splits orders evenly over time
//! - **VWAP (Volume-Weighted Average Price)**
//! - **POV (Percentage of Volume)**
//! - **Implementation Shortfall (IS)**
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

pub mod is;
pub mod pov;
pub mod twap;
pub mod vwap;

pub use is::*;
pub use pov::*;
pub use twap::*;
pub use vwap::*;
