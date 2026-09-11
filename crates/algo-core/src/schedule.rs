//! Types and helpers shared by the schedulers.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Upper bound on slices per schedule, keeping memory and run time bounded.
pub const MAX_SLICES: usize = 1_000_000;

/// One child order in a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildOrderInstruction {
    /// When to release the child order, in nanoseconds on the window's clock.
    pub target_time_ns: u64,
    /// Quantity of the child order; always greater than zero.
    pub qty: u64,
}

/// Why a schedule could not be computed.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScheduleError {
    #[error("total quantity must be greater than zero")]
    ZeroQuantity,
    #[error("number of slices must be greater than zero")]
    ZeroSlices,
    #[error("{num_slices} slices exceeds the maximum of {max}")]
    TooManySlices { num_slices: usize, max: usize },
    #[error("end time ({end_ns} ns) must be after start time ({start_ns} ns)")]
    InvalidTimeRange { start_ns: u64, end_ns: u64 },
    #[error("a {duration_ns} ns window is too short for {num_slices} slices of at least 1 ns")]
    WindowTooShort { duration_ns: u64, num_slices: usize },
    #[error("volume profile entry {index} must be finite and non-negative, got {value}")]
    InvalidProfileEntry { index: usize, value: f64 },
    #[error("volume profile contains no volume")]
    EmptyProfile,
    #[error("market volume observation {index} is earlier than the one before it")]
    UnsortedMarketData { index: usize },
    #[error("invalid {name}: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
}

/// Validates a scheduling window and returns its duration in nanoseconds.
pub(crate) fn validate_window(
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    num_slices: usize,
) -> Result<u64, ScheduleError> {
    if total_qty == 0 {
        return Err(ScheduleError::ZeroQuantity);
    }
    if num_slices == 0 {
        return Err(ScheduleError::ZeroSlices);
    }
    if num_slices > MAX_SLICES {
        return Err(ScheduleError::TooManySlices {
            num_slices,
            max: MAX_SLICES,
        });
    }
    if end_ns <= start_ns {
        return Err(ScheduleError::InvalidTimeRange { start_ns, end_ns });
    }
    let duration_ns = end_ns - start_ns;
    if duration_ns < num_slices as u64 {
        return Err(ScheduleError::WindowTooShort {
            duration_ns,
            num_slices,
        });
    }
    Ok(duration_ns)
}

/// Start of slice `index` when `duration_ns` from `start_ns` is cut into
/// `num_slices` slices whose lengths differ by at most 1 ns. An `index` of
/// `num_slices` gives the end of the window.
pub(crate) fn slice_start(start_ns: u64, duration_ns: u64, num_slices: usize, index: usize) -> u64 {
    let offset = u128::from(duration_ns) * index as u128 / num_slices as u128;
    // offset <= duration_ns, so the sum cannot pass the window end.
    start_ns + offset as u64
}

/// Integer per-slice quantities that follow a cumulative target curve.
///
/// `cumulative[k]` is the fraction of `total` that should be complete after
/// slice `k`; it should be non-decreasing and within `[0, 1]`. Each cumulative
/// target is rounded to the nearest unit and the last slice always completes
/// `total`, so the quantities sum exactly to `total` and, for totals below
/// 2^53, every prefix of the schedule is within half a unit of the ideal curve.
/// Rounding the running total, rather than each slice independently, is what
/// keeps the error from accumulating.
pub fn quantities_from_cumulative(total: u64, cumulative: &[f64]) -> Vec<u64> {
    let mut quantities = Vec::with_capacity(cumulative.len());
    let mut done = 0u64;
    for (k, &fraction) in cumulative.iter().enumerate() {
        let target = if k + 1 == cumulative.len() {
            total
        } else {
            let ideal = (total as f64 * fraction.clamp(0.0, 1.0)).round();
            // The clamp keeps the schedule monotone even if the curve or the
            // float product is slightly off (e.g. totals above 2^53).
            (ideal as u64).clamp(done, total)
        };
        quantities.push(target - done);
        done = target;
    }
    quantities
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn window_validation() {
        assert_eq!(
            validate_window(0, 10, 0, 5),
            Err(ScheduleError::ZeroQuantity)
        );
        assert_eq!(validate_window(0, 10, 1, 0), Err(ScheduleError::ZeroSlices));
        assert_eq!(
            validate_window(0, 10, 1, MAX_SLICES + 1),
            Err(ScheduleError::TooManySlices {
                num_slices: MAX_SLICES + 1,
                max: MAX_SLICES
            })
        );
        assert_eq!(
            validate_window(10, 10, 1, 1),
            Err(ScheduleError::InvalidTimeRange {
                start_ns: 10,
                end_ns: 10
            })
        );
        assert_eq!(
            validate_window(0, 4, 1, 5),
            Err(ScheduleError::WindowTooShort {
                duration_ns: 4,
                num_slices: 5
            })
        );
        assert_eq!(validate_window(0, 5, 1, 5), Ok(5));
    }

    #[test]
    fn slices_cover_the_whole_window() {
        let starts: Vec<u64> = (0..=3).map(|i| slice_start(100, 10, 3, i)).collect();
        assert_eq!(starts, vec![100, 103, 106, 110]);
        assert_eq!(slice_start(u64::MAX - 10, 10, 3, 3), u64::MAX);
    }

    #[test]
    fn cumulative_rounding_is_exact_and_spreads_remainders() {
        assert_eq!(
            quantities_from_cumulative(100, &[1.0 / 3.0, 2.0 / 3.0, 1.0]),
            vec![33, 34, 33]
        );
        assert_eq!(
            quantities_from_cumulative(10, &[0.5, 0.5, 1.0]),
            vec![5, 0, 5]
        );
        // The last slice completes the total even if the curve stops short.
        assert_eq!(quantities_from_cumulative(10, &[0.2, 0.9]), vec![2, 8]);
    }

    #[test]
    fn cumulative_rounding_handles_the_largest_totals() {
        let quantities = quantities_from_cumulative(u64::MAX, &[0.25, 0.5, 1.0]);
        let sum: u128 = quantities.iter().map(|&q| u128::from(q)).sum();
        assert_eq!(sum, u128::from(u64::MAX));
    }

    proptest! {
        #[test]
        fn cumulative_rounding_tracks_the_curve(
            total in 1u64..=1_000_000_000_000,
            weights in prop::collection::vec(0u32..1_000, 1..200),
        ) {
            let weight_total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
            prop_assume!(weight_total > 0);

            let mut running = 0u64;
            let cumulative: Vec<f64> = weights
                .iter()
                .map(|&w| {
                    running += u64::from(w);
                    running as f64 / weight_total as f64
                })
                .collect();
            let quantities = quantities_from_cumulative(total, &cumulative);

            prop_assert_eq!(quantities.iter().sum::<u64>(), total);
            let mut done = 0u64;
            for (k, &qty) in quantities.iter().enumerate() {
                done += qty;
                let ideal = total as f64 * cumulative[k];
                prop_assert!((done as f64 - ideal).abs() <= 0.5 + 1e-9 * total as f64,
                    "prefix {} is {} but the curve says {}", k, done, ideal);
                if weights[k] == 0 {
                    prop_assert_eq!(qty, 0);
                }
            }
        }
    }
}
