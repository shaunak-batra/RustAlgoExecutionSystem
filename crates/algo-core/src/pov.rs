//! Percentage-of-volume (POV) schedules.

use crate::schedule::{ChildOrderInstruction, ScheduleError};
use serde::{Deserialize, Serialize};

/// Basis points in 100%.
pub const BPS_PER_UNIT: u32 = 10_000;

/// Parameters for a POV schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PovParams {
    /// Window start (nanoseconds). Earlier observations are ignored.
    pub start_ns: u64,
    /// Window end, exclusive (nanoseconds). Later observations are ignored.
    pub end_ns: u64,
    pub total_qty: u64,
    /// Participation rate in basis points of observed volume, 1 to 10 000.
    pub participation_bps: u32,
}

/// Output of [`compute_pov_schedule`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PovSchedule {
    /// At most one child per observation timestamp, in time order.
    pub children: Vec<ChildOrderInstruction>,
    /// Sum of the children's quantities.
    pub scheduled_qty: u64,
    /// `total_qty - scheduled_qty`: quantity the observed volume did not allow.
    pub shortfall_qty: u64,
}

/// Trades a fixed share of observed market volume.
///
/// `market_volume` holds `(timestamp_ns, volume)` observations in time order,
/// where `volume` is the volume that traded *since the observation before it*
/// (an increment, not a running total) and `timestamp_ns` is when that volume
/// became known — a bar's close, not its open.
///
/// Each child order is released at the timestamp of the observation that allowed
/// it, so the schedule never reacts to an observation that appears later in the
/// data. Note what that does and does not promise: release happens at the same
/// instant as the print it reacts to, so passing timestamps that precede the
/// information (bar opens, for instance) would be look-ahead no matter what this
/// function does. Offset the timestamps if a strictly later release is wanted.
///
/// After each observation in
/// `[start_ns, end_ns)` the cumulative target is
/// `min(total_qty, ⌊cumulative volume × participation_bps / 10 000⌋)`, and a
/// child order for any increase is released at that observation's timestamp.
/// The arithmetic is exact integer arithmetic, so the cumulative scheduled
/// quantity never exceeds the rate times cumulative volume.
///
/// The rate applies to whatever volume is passed in: pass total market volume
/// (including this order's own fills) to target that share of all trading, or
/// volume excluding own fills to trade at that ratio to other participants.
///
/// # Examples
/// ```
/// use algo_core::pov::{compute_pov_schedule, PovParams};
///
/// // 10% of volume, observed in three prints.
/// let params = PovParams { start_ns: 0, end_ns: 10, total_qty: 1_000, participation_bps: 1_000 };
/// let schedule = compute_pov_schedule(params, &[(1, 105), (2, 105), (3, 90)]).unwrap();
///
/// let quantities: Vec<u64> = schedule.children.iter().map(|child| child.qty).collect();
/// assert_eq!(quantities, vec![10, 11, 9]);
/// assert_eq!(schedule.shortfall_qty, 970);
/// ```
pub fn compute_pov_schedule(
    params: PovParams,
    market_volume: &[(u64, u64)],
) -> Result<PovSchedule, ScheduleError> {
    if params.total_qty == 0 {
        return Err(ScheduleError::ZeroQuantity);
    }
    if params.end_ns <= params.start_ns {
        return Err(ScheduleError::InvalidTimeRange {
            start_ns: params.start_ns,
            end_ns: params.end_ns,
        });
    }
    if params.participation_bps == 0 || params.participation_bps > BPS_PER_UNIT {
        return Err(ScheduleError::InvalidParameter {
            name: "participation_bps",
            reason: "must be between 1 and 10000",
        });
    }
    if let Some(index) = market_volume
        .windows(2)
        .position(|pair| pair[1].0 < pair[0].0)
    {
        return Err(ScheduleError::UnsortedMarketData { index: index + 1 });
    }

    let total = u128::from(params.total_qty);
    let rate = u128::from(params.participation_bps);
    let mut cumulative_volume = 0u128;
    let mut scheduled = 0u128;
    let mut children: Vec<ChildOrderInstruction> = Vec::new();

    for &(timestamp_ns, volume) in market_volume {
        if timestamp_ns < params.start_ns {
            continue;
        }
        if timestamp_ns >= params.end_ns || scheduled == total {
            break;
        }
        cumulative_volume += u128::from(volume);
        let target = (cumulative_volume * rate / u128::from(BPS_PER_UNIT)).min(total);
        if target > scheduled {
            // The increase is at most total_qty, so it fits in u64.
            let qty = (target - scheduled) as u64;
            match children.last_mut() {
                Some(last) if last.target_time_ns == timestamp_ns => last.qty += qty,
                _ => children.push(ChildOrderInstruction {
                    target_time_ns: timestamp_ns,
                    qty,
                }),
            }
            scheduled = target;
        }
    }

    let scheduled_qty = scheduled as u64;
    Ok(PovSchedule {
        children,
        scheduled_qty,
        shortfall_qty: params.total_qty - scheduled_qty,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn params(total_qty: u64, participation_bps: u32) -> PovParams {
        PovParams {
            start_ns: 0,
            end_ns: 1_000,
            total_qty,
            participation_bps,
        }
    }

    fn pairs(schedule: &PovSchedule) -> Vec<(u64, u64)> {
        schedule
            .children
            .iter()
            .map(|child| (child.target_time_ns, child.qty))
            .collect()
    }

    #[test]
    fn participation_is_rounded_down() {
        let schedule =
            compute_pov_schedule(params(1_000, 1_000), &[(1, 105), (2, 105), (3, 90)]).unwrap();
        assert_eq!(pairs(&schedule), vec![(1, 10), (2, 11), (3, 9)]);
        assert_eq!(schedule.scheduled_qty, 30);
        assert_eq!(schedule.shortfall_qty, 970);
    }

    #[test]
    fn stops_once_the_order_is_complete() {
        let data = [(1, 20), (2, 20), (3, 20), (4, 20)];
        let schedule = compute_pov_schedule(params(25, 5_000), &data).unwrap();
        assert_eq!(pairs(&schedule), vec![(1, 10), (2, 10), (3, 5)]);
        assert_eq!(schedule.shortfall_qty, 0);
    }

    #[test]
    fn only_volume_inside_the_window_counts() {
        let p = PovParams {
            start_ns: 10,
            end_ns: 20,
            total_qty: 1_000,
            participation_bps: 10_000,
        };
        let schedule = compute_pov_schedule(p, &[(5, 100), (10, 7), (19, 3), (20, 100)]).unwrap();
        assert_eq!(pairs(&schedule), vec![(10, 7), (19, 3)]);
    }

    #[test]
    fn observations_at_the_same_time_share_one_child() {
        let data = [(1, 10), (1, 10), (2, 10)];
        let schedule = compute_pov_schedule(params(1_000, 5_000), &data).unwrap();
        assert_eq!(pairs(&schedule), vec![(1, 10), (2, 5)]);
    }

    #[test]
    fn no_volume_means_full_shortfall() {
        let schedule = compute_pov_schedule(params(10, 100), &[]).unwrap();
        assert!(schedule.children.is_empty());
        assert_eq!(schedule.shortfall_qty, 10);
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert_eq!(
            compute_pov_schedule(params(0, 100), &[]),
            Err(ScheduleError::ZeroQuantity)
        );
        assert!(matches!(
            compute_pov_schedule(params(10, 0), &[]),
            Err(ScheduleError::InvalidParameter {
                name: "participation_bps",
                ..
            })
        ));
        assert!(matches!(
            compute_pov_schedule(params(10, 10_001), &[]),
            Err(ScheduleError::InvalidParameter {
                name: "participation_bps",
                ..
            })
        ));
        let empty_window = PovParams {
            start_ns: 5,
            end_ns: 5,
            total_qty: 10,
            participation_bps: 100,
        };
        assert!(matches!(
            compute_pov_schedule(empty_window, &[]),
            Err(ScheduleError::InvalidTimeRange { .. })
        ));
        // Out-of-order data is rejected even when it lies outside the window.
        assert_eq!(
            compute_pov_schedule(params(10, 100), &[(2_000, 1), (1_500, 1)]),
            Err(ScheduleError::UnsortedMarketData { index: 1 })
        );
    }

    proptest! {
        #[test]
        fn cap_completeness_and_prefix_invariance(
            total_qty in 1u64..1_000_000_000,
            participation_bps in 1u32..=10_000,
            // Gaps of zero repeat a timestamp, and small volumes make increments
            // that round to nothing. Both are cases the schedule has to handle.
            observations in prop::collection::vec((0u64..3, 0u64..1_000_000_000), 0..60),
        ) {
            let mut timestamp = 0u64;
            let data: Vec<(u64, u64)> = observations
                .iter()
                .map(|&(gap, volume)| {
                    timestamp += gap;
                    (timestamp, volume)
                })
                .collect();
            let p = PovParams { start_ns: 0, end_ns: 1_000, total_qty, participation_bps };
            let full = compute_pov_schedule(p, &data).unwrap();

            // Every child is a real order: positive, inside the window, in strict
            // time order, and the children account for scheduled_qty exactly.
            prop_assert!(full.children.iter().all(|c| c.qty > 0));
            prop_assert!(full.children.windows(2).all(|w| w[0].target_time_ns < w[1].target_time_ns));
            prop_assert!(full.children.iter().all(|c| (p.start_ns..p.end_ns).contains(&c.target_time_ns)));
            prop_assert_eq!(
                full.children.iter().map(|c| u128::from(c.qty)).sum::<u128>(),
                u128::from(full.scheduled_qty)
            );

            // Cap: the schedule is never ahead of rate x cumulative volume.
            let mut cumulative_volume = 0u128;
            let mut done = 0u128;
            let mut children = full.children.iter().peekable();
            for (i, &(t, v)) in data.iter().enumerate() {
                cumulative_volume += u128::from(v);
                // Observations sharing a timestamp are one decision point.
                if data.get(i + 1).is_some_and(|&(next, _)| next == t) {
                    continue;
                }
                if children.peek().is_some_and(|c| c.target_time_ns == t) {
                    done += u128::from(children.next().unwrap().qty);
                }
                prop_assert!(done * 10_000 <= cumulative_volume * u128::from(participation_bps));
            }

            // Completeness: it schedules everything the cap allows.
            let allowed = (cumulative_volume * u128::from(participation_bps) / 10_000)
                .min(u128::from(total_qty));
            prop_assert_eq!(u128::from(full.scheduled_qty), allowed);
            prop_assert_eq!(full.scheduled_qty + full.shortfall_qty, total_qty);

            // Prefix invariance: once every observation carrying a timestamp has
            // been seen, that timestamp's child is settled and later data cannot
            // change it.
            for cut in 0..=data.len() {
                let prefix = compute_pov_schedule(p, &data[..cut]).unwrap();
                let boundary = data.get(cut).map_or(u64::MAX, |&(t, _)| t);
                let settled = |children: &[ChildOrderInstruction]| -> Vec<ChildOrderInstruction> {
                    children.iter().copied().filter(|c| c.target_time_ns < boundary).collect()
                };
                prop_assert_eq!(settled(&prefix.children), settled(&full.children));
            }
        }
    }
}
