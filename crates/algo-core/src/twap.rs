use serde::{Deserialize, Serialize};

/// Parameters for TWAP (Time-Weighted Average Price) execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TwapParams {
    /// Start time in nanoseconds since epoch
    pub start_ns: u64,
    /// End time in nanoseconds since epoch
    pub end_ns: u64,
    /// Total quantity to execute
    pub total_qty: u64,
    /// Number of time slices to split the order into
    pub num_slices: usize,
}

/// A single child order instruction from the TWAP schedule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildOrderInstruction {
    /// Target execution time in nanoseconds
    pub target_time_ns: u64,
    /// Quantity for this child order
    pub qty: u64,
}

/// Pure function: compute TWAP schedule. No I/O, no side effects.
/// This function is FFI-safe: all types are repr(C) compatible or plain data.
///
/// # Arguments
/// * `params` - TWAP parameters including time range, quantity, and number of slices
///
/// # Returns
/// Vector of child order instructions, each with a target time and quantity.
///
/// # Examples
/// ```
/// use algo_core::twap::{TwapParams, compute_twap_schedule};
///
/// let params = TwapParams {
///     start_ns: 0,
///     end_ns: 10_000_000_000, // 10 seconds
///     total_qty: 1000,
///     num_slices: 10,
/// };
///
/// let schedule = compute_twap_schedule(params);
/// assert_eq!(schedule.len(), 10);
/// assert_eq!(schedule.iter().map(|s| s.qty).sum::<u64>(), 1000);
/// ```
pub fn compute_twap_schedule(params: TwapParams) -> Vec<ChildOrderInstruction> {
    // Validate parameters
    if params.num_slices == 0 {
        return Vec::new();
    }

    if params.total_qty == 0 {
        return Vec::new();
    }

    if params.end_ns <= params.start_ns {
        return Vec::new();
    }

    let duration_ns = params.end_ns.saturating_sub(params.start_ns);
    let slice_duration_ns = duration_ns / params.num_slices as u64;

    // Calculate base quantity per slice and remainder
    let base_qty = params.total_qty / params.num_slices as u64;
    let remainder = params.total_qty % params.num_slices as u64;

    // Generate schedule: distribute remainder evenly across first N slices
    (0..params.num_slices)
        .map(|i| {
            let target_time_ns = params.start_ns + i as u64 * slice_duration_ns;
            // Distribute remainder to first slices
            let qty = base_qty + if i < remainder as usize { 1 } else { 0 };
            ChildOrderInstruction { target_time_ns, qty }
        })
        .collect()
}

/// Variant: TWAP with randomized execution times within each slice window
/// (for market impact reduction)
pub fn compute_twap_randomized(
    params: TwapParams,
    seed: u64,
) -> Vec<ChildOrderInstruction> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let base_schedule = compute_twap_schedule(params);

    if base_schedule.is_empty() {
        return base_schedule;
    }

    let duration_ns = params.end_ns.saturating_sub(params.start_ns);
    let slice_duration_ns = duration_ns / params.num_slices as u64;

    // Apply jitter to each target time (within slice window)
    base_schedule
        .into_iter()
        .enumerate()
        .map(|(i, mut instr)| {
            // Simple deterministic pseudo-random jitter using hash
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u64(seed);
            hasher.write_usize(i);
            let random_value = hasher.finish();

            // Jitter within ±25% of slice duration
            let max_jitter = slice_duration_ns / 4;
            let jitter = (random_value % (2 * max_jitter)).saturating_sub(max_jitter);

            instr.target_time_ns = instr
                .target_time_ns
                .saturating_add(jitter)
                .max(params.start_ns)
                .min(params.end_ns);

            instr
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twap_even_split() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 100,
            num_slices: 5,
        };
        let schedule = compute_twap_schedule(params);

        assert_eq!(schedule.len(), 5);
        assert_eq!(schedule.iter().map(|s| s.qty).sum::<u64>(), 100);

        // Each slice should have 20 qty
        for instr in &schedule {
            assert_eq!(instr.qty, 20);
        }

        // Check timing
        assert_eq!(schedule[0].target_time_ns, 0);
        assert_eq!(schedule[1].target_time_ns, 2_000);
        assert_eq!(schedule[4].target_time_ns, 8_000);
    }

    #[test]
    fn test_twap_with_remainder() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 103,
            num_slices: 5,
        };
        let schedule = compute_twap_schedule(params);

        assert_eq!(schedule.len(), 5);
        assert_eq!(schedule.iter().map(|s| s.qty).sum::<u64>(), 103);

        // First 3 slices get 21, last 2 get 20
        assert_eq!(schedule[0].qty, 21);
        assert_eq!(schedule[1].qty, 21);
        assert_eq!(schedule[2].qty, 21);
        assert_eq!(schedule[3].qty, 20);
        assert_eq!(schedule[4].qty, 20);
    }

    #[test]
    fn test_twap_single_slice() {
        let params = TwapParams {
            start_ns: 1000,
            end_ns: 5000,
            total_qty: 50,
            num_slices: 1,
        };
        let schedule = compute_twap_schedule(params);

        assert_eq!(schedule.len(), 1);
        assert_eq!(schedule[0].qty, 50);
        assert_eq!(schedule[0].target_time_ns, 1000);
    }

    #[test]
    fn test_twap_zero_quantity() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 0,
            num_slices: 5,
        };
        let schedule = compute_twap_schedule(params);

        assert!(schedule.is_empty());
    }

    #[test]
    fn test_twap_zero_slices() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 100,
            num_slices: 0,
        };
        let schedule = compute_twap_schedule(params);

        assert!(schedule.is_empty());
    }

    #[test]
    fn test_twap_invalid_time_range() {
        let params = TwapParams {
            start_ns: 10_000,
            end_ns: 5_000, // end before start!
            total_qty: 100,
            num_slices: 5,
        };
        let schedule = compute_twap_schedule(params);

        assert!(schedule.is_empty());
    }

    #[test]
    fn test_twap_large_quantities() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 1_000_000_000, // 1 second
            total_qty: 1_000_000,
            num_slices: 100,
        };
        let schedule = compute_twap_schedule(params);

        assert_eq!(schedule.len(), 100);
        assert_eq!(schedule.iter().map(|s| s.qty).sum::<u64>(), 1_000_000);

        // Each slice should be 10,000
        for instr in &schedule {
            assert_eq!(instr.qty, 10_000);
        }
    }

    #[test]
    fn test_twap_randomized_deterministic() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 100,
            num_slices: 5,
        };

        let schedule1 = compute_twap_randomized(params, 42);
        let schedule2 = compute_twap_randomized(params, 42);

        // Same seed should produce same schedule
        assert_eq!(schedule1, schedule2);

        // Quantities should still sum correctly
        assert_eq!(schedule1.iter().map(|s| s.qty).sum::<u64>(), 100);
    }

    #[test]
    fn test_twap_randomized_different_seeds() {
        let params = TwapParams {
            start_ns: 0,
            end_ns: 10_000,
            total_qty: 100,
            num_slices: 5,
        };

        let schedule1 = compute_twap_randomized(params, 42);
        let schedule2 = compute_twap_randomized(params, 123);

        // Different seeds should produce different timings (likely)
        let timings_equal = schedule1
            .iter()
            .zip(&schedule2)
            .all(|(a, b)| a.target_time_ns == b.target_time_ns);

        assert!(!timings_equal, "Different seeds should produce different timings");

        // But quantities should match
        for (a, b) in schedule1.iter().zip(&schedule2) {
            assert_eq!(a.qty, b.qty);
        }
    }

    #[test]
    fn test_twap_randomized_bounds() {
        let params = TwapParams {
            start_ns: 1_000,
            end_ns: 11_000,
            total_qty: 50,
            num_slices: 5,
        };

        let schedule = compute_twap_randomized(params, 999);

        // All times should be within bounds
        for instr in &schedule {
            assert!(
                instr.target_time_ns >= params.start_ns,
                "Time {} is before start {}",
                instr.target_time_ns,
                params.start_ns
            );
            assert!(
                instr.target_time_ns <= params.end_ns,
                "Time {} is after end {}",
                instr.target_time_ns,
                params.end_ns
            );
        }
    }
}
