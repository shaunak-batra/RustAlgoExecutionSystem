//! Time-weighted average price (TWAP) schedules.

use crate::schedule::{slice_start, validate_window, ChildOrderInstruction, ScheduleError};
use serde::{Deserialize, Serialize};

/// Parameters for a TWAP schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwapParams {
    /// Window start (nanoseconds).
    pub start_ns: u64,
    /// Window end, exclusive (nanoseconds).
    pub end_ns: u64,
    pub total_qty: u64,
    pub num_slices: usize,
}

/// Splits `total_qty` evenly over `num_slices` equal time slices of
/// `[start_ns, end_ns)`, one child order at the start of each slice.
///
/// Slice `k` starts at `start_ns + ⌊k · duration / num_slices⌋`. The quantity
/// complete after slice `k` is `total_qty · (k + 1) / num_slices` rounded to
/// the nearest unit (ties round up), so slice quantities differ by at most
/// one, any remainder is spread across the window instead of front-loaded,
/// and the total is exact. Slices that round to zero (possible only when
/// `total_qty < num_slices`) are omitted. Integer arithmetic throughout.
///
/// # Examples
/// ```
/// use algo_core::twap::{compute_twap_schedule, TwapParams};
///
/// let schedule = compute_twap_schedule(TwapParams {
///     start_ns: 0,
///     end_ns: 5_000_000_000,
///     total_qty: 103,
///     num_slices: 5,
/// })
/// .unwrap();
///
/// let quantities: Vec<u64> = schedule.iter().map(|child| child.qty).collect();
/// assert_eq!(quantities, vec![21, 20, 21, 20, 21]);
/// assert_eq!(schedule[1].target_time_ns, 1_000_000_000);
/// ```
pub fn compute_twap_schedule(
    params: TwapParams,
) -> Result<Vec<ChildOrderInstruction>, ScheduleError> {
    let duration_ns = validate_window(
        params.start_ns,
        params.end_ns,
        params.total_qty,
        params.num_slices,
    )?;
    Ok(twap_quantities(params.total_qty, params.num_slices)
        .enumerate()
        .filter(|&(_, qty)| qty > 0)
        .map(|(index, qty)| ChildOrderInstruction {
            target_time_ns: slice_start(params.start_ns, duration_ns, params.num_slices, index),
            qty,
        })
        .collect())
}

/// TWAP with each child order released at a pseudo-random time inside its slice.
///
/// Quantities are identical to [`compute_twap_schedule`]. The release time for
/// slice `k` is drawn uniformly from `[start of slice k, start of slice k + 1)`
/// with SplitMix64 seeded by `seed`: the same seed gives the same schedule on
/// every platform and Rust version, while an observer cannot predict release
/// times from a fixed clock. One draw is made per slice, including a slice whose
/// quantity rounds to zero, so slice `k`'s release time depends only on the
/// seed, the window, `num_slices` and `k` — never on `total_qty`.
pub fn compute_twap_randomized(
    params: TwapParams,
    seed: u64,
) -> Result<Vec<ChildOrderInstruction>, ScheduleError> {
    let duration_ns = validate_window(
        params.start_ns,
        params.end_ns,
        params.total_qty,
        params.num_slices,
    )?;
    let mut rng = SplitMix64::new(seed);
    let mut schedule = Vec::with_capacity(params.num_slices);

    for (index, qty) in twap_quantities(params.total_qty, params.num_slices).enumerate() {
        let from = slice_start(params.start_ns, duration_ns, params.num_slices, index);
        let to = slice_start(params.start_ns, duration_ns, params.num_slices, index + 1);
        // Every slice is at least 1 ns long because the window was validated.
        let offset = rng.below(to - from);
        if qty > 0 {
            schedule.push(ChildOrderInstruction {
                target_time_ns: from + offset,
                qty,
            });
        }
    }
    Ok(schedule)
}

/// Per-slice TWAP quantities (including zeros): the cumulative target
/// `total · k / n` rounded to the nearest integer, computed as
/// `q·k + round(r·k / n)` with `total = q·n + r` so nothing overflows.
pub(crate) fn twap_quantities(total_qty: u64, num_slices: usize) -> impl Iterator<Item = u64> {
    let n = num_slices as u128;
    let (quotient, remainder) = (u128::from(total_qty) / n, u128::from(total_qty) % n);
    let mut done = 0u128;
    (1..=n).map(move |k| {
        let partial = remainder * k;
        let target = quotient * k + partial / n + u128::from(2 * (partial % n) >= n);
        let qty = (target - done) as u64;
        done = target;
        qty
    })
}

/// SplitMix64 pseudo-random generator (the widely used `splitmix64.c`
/// variant of Steele, Lea & Flood, 2014). Its output is fixed by the
/// algorithm, unlike `std`'s `DefaultHasher`, whose algorithm is unspecified
/// and may change between Rust releases.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `[0, bound)` via multiply-shift (bias below `bound / 2^64`).
    fn below(&mut self, bound: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(bound)) >> 64) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn params(start_ns: u64, end_ns: u64, total_qty: u64, num_slices: usize) -> TwapParams {
        TwapParams {
            start_ns,
            end_ns,
            total_qty,
            num_slices,
        }
    }

    fn quantities(schedule: &[ChildOrderInstruction]) -> Vec<u64> {
        schedule.iter().map(|child| child.qty).collect()
    }

    fn times(schedule: &[ChildOrderInstruction]) -> Vec<u64> {
        schedule.iter().map(|child| child.target_time_ns).collect()
    }

    #[test]
    fn even_split_at_slice_starts() {
        let schedule = compute_twap_schedule(params(0, 10_000, 100, 5)).unwrap();
        assert_eq!(quantities(&schedule), vec![20; 5]);
        assert_eq!(times(&schedule), vec![0, 2_000, 4_000, 6_000, 8_000]);
    }

    #[test]
    fn remainder_is_spread_across_the_window() {
        let schedule = compute_twap_schedule(params(0, 10_000, 103, 5)).unwrap();
        assert_eq!(quantities(&schedule), vec![21, 20, 21, 20, 21]);
    }

    #[test]
    fn uneven_durations_still_cover_the_window() {
        let schedule = compute_twap_schedule(params(0, 10, 3, 3)).unwrap();
        assert_eq!(times(&schedule), vec![0, 3, 6]);
    }

    #[test]
    fn fewer_units_than_slices_are_spaced_out() {
        let schedule = compute_twap_schedule(params(0, 10_000, 5, 10)).unwrap();
        assert_eq!(quantities(&schedule), vec![1; 5]);
        assert_eq!(times(&schedule), vec![0, 2_000, 4_000, 6_000, 8_000]);
    }

    #[test]
    fn single_slice() {
        let schedule = compute_twap_schedule(params(1_000, 5_000, 50, 1)).unwrap();
        assert_eq!(
            schedule,
            vec![ChildOrderInstruction {
                target_time_ns: 1_000,
                qty: 50
            }]
        );
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        assert_eq!(
            compute_twap_schedule(params(0, 10_000, 0, 5)),
            Err(ScheduleError::ZeroQuantity)
        );
        assert_eq!(
            compute_twap_schedule(params(0, 10_000, 100, 0)),
            Err(ScheduleError::ZeroSlices)
        );
        assert!(matches!(
            compute_twap_schedule(params(10_000, 5_000, 100, 5)),
            Err(ScheduleError::InvalidTimeRange { .. })
        ));
        assert!(matches!(
            compute_twap_randomized(params(0, 4, 100, 5), 1),
            Err(ScheduleError::WindowTooShort { .. })
        ));
    }

    #[test]
    fn largest_quantities_do_not_overflow() {
        let schedule =
            compute_twap_schedule(params(0, u64::MAX, u64::MAX, crate::MAX_SLICES)).unwrap();
        let total: u128 = schedule.iter().map(|child| u128::from(child.qty)).sum();
        assert_eq!(total, u128::from(u64::MAX));
    }

    #[test]
    fn splitmix64_matches_reference_output() {
        // First output of splitmix64.c seeded with 0.
        assert_eq!(SplitMix64::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn randomized_times_stay_inside_their_slices() {
        let p = params(1_000, 11_000, 50, 5);
        let plain = compute_twap_schedule(p).unwrap();
        for seed in 0..200 {
            let randomized = compute_twap_randomized(p, seed).unwrap();
            assert_eq!(quantities(&randomized), quantities(&plain));
            for (k, child) in randomized.iter().enumerate() {
                let from = 1_000 + 2_000 * k as u64;
                assert!((from..from + 2_000).contains(&child.target_time_ns));
            }
        }
    }

    #[test]
    fn randomized_schedule_matches_pinned_values() {
        // Pinned against an independent SplitMix64 implementation. The window is
        // not divisible by the slice count (10_000 ns over 7), so slice lengths
        // differ, and total_qty < num_slices, so some slices round to zero.
        let schedule = compute_twap_randomized(params(1_000, 11_000, 5, 7), 42).unwrap();
        let pairs: Vec<(u64, u64)> = schedule
            .iter()
            .map(|child| (child.target_time_ns, child.qty))
            .collect();
        assert_eq!(
            pairs,
            vec![(2_058, 1), (4_254, 1), (5_776, 1), (6_768, 1), (9_883, 1)]
        );
    }

    #[test]
    fn slice_release_times_do_not_depend_on_the_quantity() {
        // A slice that rounds to zero still consumes its draw, so the slices that
        // do trade keep the times they would have had.
        let p = |total_qty| params(1_000, 11_000, total_qty, 7);
        let sparse = compute_twap_randomized(p(5), 42).unwrap();
        let dense = compute_twap_randomized(p(100), 42).unwrap();
        assert_eq!(quantities(&sparse), vec![1; 5]);
        assert_eq!(quantities(&dense), vec![14, 15, 14, 14, 14, 15, 14]);

        let dense_times = times(&dense);
        for time in times(&sparse) {
            assert!(dense_times.contains(&time), "slice time {time} moved");
        }
    }

    #[test]
    fn randomized_is_deterministic_per_seed() {
        let p = params(0, 10_000_000_000, 1_000, 10);
        assert_eq!(
            compute_twap_randomized(p, 42).unwrap(),
            compute_twap_randomized(p, 42).unwrap()
        );
        assert_ne!(
            times(&compute_twap_randomized(p, 42).unwrap()),
            times(&compute_twap_randomized(p, 43).unwrap())
        );
    }

    #[test]
    fn randomized_offsets_are_roughly_uniform() {
        // 100 slices of 1000 ns over 200 seeds: 20,000 draws, mean offset near 499.5.
        let p = params(0, 100_000, 100, 100);
        let mut sum = 0u64;
        let mut count = 0u64;
        let mut early = 0u64;
        for seed in 0..200 {
            for (k, child) in compute_twap_randomized(p, seed).unwrap().iter().enumerate() {
                let offset = child.target_time_ns - 1_000 * k as u64;
                sum += offset;
                count += 1;
                early += u64::from(offset < 500);
            }
        }
        let mean = sum as f64 / count as f64;
        assert!((mean - 499.5).abs() < 10.0, "mean offset {mean}");
        let early_share = early as f64 / count as f64;
        assert!(
            (early_share - 0.5).abs() < 0.02,
            "share in first half {early_share}"
        );
    }

    proptest! {
        #[test]
        fn schedule_invariants(
            start_ns in 0u64..1_000_000_000_000,
            extra_ns in 0u64..1_000_000_000_000,
            total_qty in 1u64..=u64::MAX,
            num_slices in 1usize..400,
            seed in any::<u64>(),
        ) {
            let end_ns = start_ns + num_slices as u64 + extra_ns;
            let p = params(start_ns, end_ns, total_qty, num_slices);
            let schedule = compute_twap_schedule(p).unwrap();

            let total: u128 = schedule.iter().map(|c| u128::from(c.qty)).sum();
            prop_assert_eq!(total, u128::from(total_qty));
            prop_assert!(schedule.iter().all(|c| c.qty > 0));
            prop_assert!(schedule.windows(2).all(|w| w[0].target_time_ns < w[1].target_time_ns));
            prop_assert!(schedule.iter().all(|c| (start_ns..end_ns).contains(&c.target_time_ns)));
            if total_qty >= num_slices as u64 {
                let max = schedule.iter().map(|c| c.qty).max().unwrap();
                let min = schedule.iter().map(|c| c.qty).min().unwrap();
                prop_assert!(max - min <= 1);
            }

            // Every prefix is within half a unit of total * k / n (checked exactly in integers).
            let n = num_slices as u128;
            let mut done = 0u128;
            let mut by_slice = schedule.iter().peekable();
            for k in 0..num_slices {
                let slice_begin = slice_start(start_ns, end_ns - start_ns, num_slices, k);
                if by_slice.peek().is_some_and(|c| c.target_time_ns == slice_begin) {
                    done += u128::from(by_slice.next().unwrap().qty);
                }
                let ideal_times_2n = 2 * u128::from(total_qty) * (k as u128 + 1);
                prop_assert!((2 * n * done).abs_diff(ideal_times_2n) <= n);
            }

            let randomized = compute_twap_randomized(p, seed).unwrap();
            prop_assert_eq!(quantities(&randomized), quantities(&schedule));
            prop_assert!(randomized.iter().all(|c| (start_ns..end_ns).contains(&c.target_time_ns)));
        }
    }
}
