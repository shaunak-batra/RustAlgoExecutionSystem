//! Volume-weighted average price (VWAP) schedules.
//!
//! A VWAP order trades in line with market volume so that its average price
//! tracks the market's VWAP over the window. Future volume is unknown, so the
//! schedule follows a volume profile estimated from history
//! (see [`volume_profile_from_bars`]).

use crate::schedule::{
    quantities_from_cumulative, slice_start, validate_window, ChildOrderInstruction, ScheduleError,
    MAX_SLICES,
};
use serde::{Deserialize, Serialize};

/// Parameters for a VWAP schedule. The number of slices is the length of the
/// volume profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VwapParams {
    /// Window start (nanoseconds).
    pub start_ns: u64,
    /// Window end, exclusive (nanoseconds).
    pub end_ns: u64,
    pub total_qty: u64,
}

/// Splits `total_qty` over equal time slices of `[start_ns, end_ns)` in
/// proportion to `volume_profile`, which holds one non-negative weight per
/// slice (any scale; the fractions from [`volume_profile_from_bars`] work).
///
/// The quantity complete after slice `k` is the profile's cumulative share
/// times `total_qty`, rounded to the nearest unit, so the total is exact and
/// every prefix stays within half a unit of the ideal curve (see
/// [`quantities_from_cumulative`]). Slices with zero weight get no child order.
///
/// # Examples
/// ```
/// use algo_core::vwap::{compute_vwap_schedule, VwapParams};
///
/// let params = VwapParams { start_ns: 0, end_ns: 2_000, total_qty: 1_000 };
/// let schedule = compute_vwap_schedule(params, &[9.0, 1.0]).unwrap();
///
/// assert_eq!((schedule[0].target_time_ns, schedule[0].qty), (0, 900));
/// assert_eq!((schedule[1].target_time_ns, schedule[1].qty), (1_000, 100));
/// ```
pub fn compute_vwap_schedule(
    params: VwapParams,
    volume_profile: &[f64],
) -> Result<Vec<ChildOrderInstruction>, ScheduleError> {
    let num_slices = volume_profile.len();
    let duration_ns =
        validate_window(params.start_ns, params.end_ns, params.total_qty, num_slices)?;
    let largest = largest_weight(volume_profile)?;

    // Dividing by the largest weight keeps the running sum finite for any finite input.
    let total_weight: f64 = volume_profile.iter().map(|w| w / largest).sum();
    let last_slice_with_volume = volume_profile.iter().rposition(|&w| w > 0.0).unwrap_or(0);
    let mut running = 0.0;
    let cumulative: Vec<f64> = volume_profile
        .iter()
        .enumerate()
        .map(|(k, &w)| {
            running += w / largest;
            // The order must be complete by the last slice that has volume.
            if k >= last_slice_with_volume {
                1.0
            } else {
                running / total_weight
            }
        })
        .collect();

    Ok(quantities_from_cumulative(params.total_qty, &cumulative)
        .into_iter()
        .enumerate()
        .filter(|&(_, qty)| qty > 0)
        .map(|(index, qty)| ChildOrderInstruction {
            target_time_ns: slice_start(params.start_ns, duration_ns, num_slices, index),
            qty,
        })
        .collect())
}

/// A daily trading window, given as offsets from the start of each day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntradayWindow {
    /// Offset of the window start from the start of the day (nanoseconds).
    pub start_offset_ns: u64,
    /// Window length (nanoseconds).
    pub duration_ns: u64,
    /// Length of a day on the timestamps' clock (nanoseconds). For Unix epoch
    /// timestamps this is [`IntradayWindow::NANOS_PER_DAY`] and days start at
    /// midnight UTC.
    pub day_ns: u64,
}

impl IntradayWindow {
    pub const NANOS_PER_DAY: u64 = 86_400_000_000_000;
}

/// Estimates a volume profile for `num_slices` equal slices of an intraday window.
///
/// Each bar is `(timestamp_ns, volume)`, with the timestamp at the start of
/// the bar. A bar belongs to the slice containing its time of day
/// (`timestamp_ns % day_ns`); bars outside the window are ignored. Volume is
/// pooled across all days and returned as each slice's fraction of the pooled
/// window volume, so the fractions sum to 1 and busier days weigh more.
///
/// Pass only bars from before the day being traded: using that day's own
/// volume would be look-ahead.
pub fn volume_profile_from_bars(
    bars: &[(u64, u64)],
    window: IntradayWindow,
    num_slices: usize,
) -> Result<Vec<f64>, ScheduleError> {
    if num_slices == 0 {
        return Err(ScheduleError::ZeroSlices);
    }
    if num_slices > MAX_SLICES {
        return Err(ScheduleError::TooManySlices {
            num_slices,
            max: MAX_SLICES,
        });
    }
    if window.day_ns == 0 {
        return Err(ScheduleError::InvalidParameter {
            name: "day_ns",
            reason: "must be greater than zero",
        });
    }
    if window.duration_ns < num_slices as u64 {
        return Err(ScheduleError::WindowTooShort {
            duration_ns: window.duration_ns,
            num_slices,
        });
    }
    let window_end = window.start_offset_ns.checked_add(window.duration_ns);
    if window_end.is_none_or(|end| end > window.day_ns) {
        return Err(ScheduleError::InvalidParameter {
            name: "window",
            reason: "must fit within one day (windows that cross midnight are not supported)",
        });
    }

    let mut volume_by_slice = vec![0u128; num_slices];
    for &(timestamp_ns, volume) in bars {
        let time_of_day = timestamp_ns % window.day_ns;
        let Some(into_window) = time_of_day.checked_sub(window.start_offset_ns) else {
            continue;
        };
        if into_window >= window.duration_ns {
            continue;
        }
        let slice = u128::from(into_window) * num_slices as u128 / u128::from(window.duration_ns);
        volume_by_slice[slice as usize] += u128::from(volume);
    }

    let total: u128 = volume_by_slice.iter().sum();
    if total == 0 {
        return Err(ScheduleError::EmptyProfile);
    }
    Ok(volume_by_slice
        .iter()
        .map(|&volume| volume as f64 / total as f64)
        .collect())
}

/// Largest weight in a profile, after checking every weight is finite and non-negative.
fn largest_weight(profile: &[f64]) -> Result<f64, ScheduleError> {
    let mut largest = 0.0_f64;
    for (index, &value) in profile.iter().enumerate() {
        if !value.is_finite() || value < 0.0 {
            return Err(ScheduleError::InvalidProfileEntry { index, value });
        }
        largest = largest.max(value);
    }
    if largest > 0.0 {
        Ok(largest)
    } else {
        Err(ScheduleError::EmptyProfile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SECOND: u64 = 1_000_000_000;

    fn params(total_qty: u64, num_slices: usize) -> VwapParams {
        VwapParams {
            start_ns: 0,
            end_ns: num_slices as u64 * SECOND,
            total_qty,
        }
    }

    fn pairs(schedule: &[ChildOrderInstruction]) -> Vec<(u64, u64)> {
        schedule
            .iter()
            .map(|child| (child.target_time_ns, child.qty))
            .collect()
    }

    #[test]
    fn quantities_follow_the_profile() {
        let schedule = compute_vwap_schedule(params(1_000, 2), &[9.0, 1.0]).unwrap();
        assert_eq!(pairs(&schedule), vec![(0, 900), (SECOND, 100)]);
    }

    #[test]
    fn equal_profile_spreads_the_remainder() {
        let schedule = compute_vwap_schedule(params(100, 3), &[1.0, 1.0, 1.0]).unwrap();
        assert_eq!(
            pairs(&schedule),
            vec![(0, 33), (SECOND, 34), (2 * SECOND, 33)]
        );
    }

    #[test]
    fn slices_without_volume_get_no_orders() {
        let schedule = compute_vwap_schedule(params(100, 5), &[0.0, 1.0, 0.0, 1.0, 0.0]).unwrap();
        assert_eq!(pairs(&schedule), vec![(SECOND, 50), (3 * SECOND, 50)]);
    }

    #[test]
    fn profile_scale_does_not_matter_even_near_f64_max() {
        let small = compute_vwap_schedule(params(1_000, 3), &[1.0, 2.0, 1.0]).unwrap();
        let huge = compute_vwap_schedule(
            params(1_000, 3),
            &[f64::MAX / 2.0, f64::MAX, f64::MAX / 2.0],
        )
        .unwrap();
        assert_eq!(
            pairs(&small),
            vec![(0, 250), (SECOND, 500), (2 * SECOND, 250)]
        );
        assert_eq!(small, huge);
    }

    #[test]
    fn invalid_profiles_are_rejected() {
        assert_eq!(
            compute_vwap_schedule(params(100, 1), &[]),
            Err(ScheduleError::ZeroSlices)
        );
        assert_eq!(
            compute_vwap_schedule(params(100, 2), &[0.0, 0.0]),
            Err(ScheduleError::EmptyProfile)
        );
        assert!(matches!(
            compute_vwap_schedule(params(100, 2), &[1.0, -1.0]),
            Err(ScheduleError::InvalidProfileEntry { index: 1, .. })
        ));
        assert!(matches!(
            compute_vwap_schedule(params(100, 2), &[f64::NAN, 1.0]),
            Err(ScheduleError::InvalidProfileEntry { index: 0, .. })
        ));
        assert!(matches!(
            compute_vwap_schedule(params(100, 1), &[f64::INFINITY]),
            Err(ScheduleError::InvalidProfileEntry { index: 0, .. })
        ));
        assert_eq!(
            compute_vwap_schedule(params(0, 1), &[1.0]),
            Err(ScheduleError::ZeroQuantity)
        );
    }

    #[test]
    fn profile_from_bars_pools_days_by_time_of_day() {
        let day = IntradayWindow::NANOS_PER_DAY;
        let open = 9 * 3_600 * SECOND;
        let window = IntradayWindow {
            start_offset_ns: open,
            duration_ns: 3_600 * SECOND,
            day_ns: day,
        };
        let bars = [
            (open, 30),
            (open + 1_800 * SECOND, 10),
            (day + open + 60 * SECOND, 50),
            (day + open + 3_000 * SECOND, 10),
            (day + 20 * 3_600 * SECOND, 1_000), // outside the window
            (open - 1, 1_000),                  // just before the window
            (open + 3_600 * SECOND, 1_000),     // the window end is exclusive
        ];
        assert_eq!(
            volume_profile_from_bars(&bars, window, 2).unwrap(),
            vec![0.8, 0.2]
        );
    }

    #[test]
    fn profile_from_bars_validates_its_window() {
        let window = |start_offset_ns, duration_ns, day_ns| IntradayWindow {
            start_offset_ns,
            duration_ns,
            day_ns,
        };
        assert!(matches!(
            volume_profile_from_bars(&[(0, 1)], window(0, 10, 0), 1),
            Err(ScheduleError::InvalidParameter { name: "day_ns", .. })
        ));
        assert!(matches!(
            volume_profile_from_bars(&[(0, 1)], window(90, 20, 100), 1),
            Err(ScheduleError::InvalidParameter { name: "window", .. })
        ));
        assert!(matches!(
            volume_profile_from_bars(&[(0, 1)], window(0, 3, 100), 4),
            Err(ScheduleError::WindowTooShort { .. })
        ));
        assert_eq!(
            volume_profile_from_bars(&[(50, 1)], window(0, 10, 100), 2),
            Err(ScheduleError::EmptyProfile)
        );
        assert_eq!(
            volume_profile_from_bars(&[(0, 1)], window(0, 10, 100), 0),
            Err(ScheduleError::ZeroSlices)
        );
    }

    proptest! {
        #[test]
        fn schedule_tracks_the_profile(
            total_qty in 1u64..=1_000_000_000_000,
            profile in prop::collection::vec(prop_oneof![1 => Just(0.0), 4 => 0.0f64..1e6], 1..100),
        ) {
            prop_assume!(profile.iter().any(|&w| w > 0.0));
            let schedule = compute_vwap_schedule(params(total_qty, profile.len()), &profile).unwrap();
            prop_assert_eq!(schedule.iter().map(|c| c.qty).sum::<u64>(), total_qty);

            let weight_total: f64 = profile.iter().sum();
            let mut done = 0u64;
            let mut running = 0.0;
            let mut children = schedule.iter().peekable();
            for (k, &weight) in profile.iter().enumerate() {
                if children.peek().is_some_and(|c| c.target_time_ns == k as u64 * SECOND) {
                    let child = children.next().unwrap();
                    prop_assert!(weight > 0.0, "slice {} has no volume but got {} units", k, child.qty);
                    done += child.qty;
                }
                running += weight;
                let ideal = total_qty as f64 * running / weight_total;
                prop_assert!((done as f64 - ideal).abs() <= 0.5 + 1e-9 * total_qty as f64,
                    "after slice {}: {} done, ideal {}", k, done, ideal);
            }
        }
    }
}
