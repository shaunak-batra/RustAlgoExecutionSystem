//! # Execution Algorithms
//!
//! Schedulers that split a parent order into child orders:
//!
//! - [`twap`]: quantities per time slice that differ by at most one unit,
//!   optionally released at a seeded random time inside each slice
//! - [`vwap`]: quantities proportional to a historical intraday volume profile
//! - [`pov`]: a fixed share of observed market volume, each child released at
//!   the timestamp of the observation that allowed it
//! - [`is`]: implementation shortfall via the Almgren–Chriss optimal trajectory
//!
//! Every scheduler validates its input and returns a [`ScheduleError`] rather
//! than panicking or silently returning an empty schedule. Quantities are
//! integers that sum exactly to the order size (for POV, to what the observed
//! volume allows).
//!
//! ## Example
//!
//! ```
//! use algo_core::{compute_twap_schedule, TwapParams};
//!
//! let schedule = compute_twap_schedule(TwapParams {
//!     start_ns: 0,
//!     end_ns: 10_000_000_000, // 10 seconds
//!     total_qty: 1_000,
//!     num_slices: 10,
//! })
//! .unwrap();
//!
//! assert_eq!(schedule.len(), 10);
//! assert!(schedule.iter().all(|child| child.qty == 100));
//! ```

pub mod is;
pub mod pov;
pub mod schedule;
pub mod twap;
pub mod vwap;

pub use is::*;
pub use pov::*;
pub use schedule::*;
pub use twap::*;
pub use vwap::*;
