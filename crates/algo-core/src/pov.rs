use serde::{Deserialize, Serialize};

/// Parameters for POV (Percent-of-Volume) execution.
/// POV aims to execute a target percentage of market volume.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PovParams {
    /// Start time in nanoseconds since epoch
    pub start_ns: u64,
    /// End time in nanoseconds since epoch
    pub end_ns: u64,
    /// Total quantity to execute
    pub total_qty: u64,
    /// Target percentage of volume (0.0 to 1.0)
    pub target_pct: f64,
    /// Maximum number of slices
    pub num_slices: usize,
}

/// POV child order instruction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PovOrderInstruction {
    /// Target execution time in nanoseconds
    pub target_time_ns: u64,
    /// Quantity for this child order (dynamically calculated)
    pub qty: u64,
    /// Observed market volume at this point
    pub market_volume: u64,
}

pub fn compute_pov_schedule(
    params: PovParams,
    market_volumes: &[(u64, u64)],
) -> Vec<PovOrderInstruction> {
    if params.num_slices == 0 || params.total_qty == 0 || market_volumes.is_empty() {
        return Vec::new();
    }

    let mut instructions = Vec::new();
    let mut remaining_qty = params.total_qty;

    for (timestamp_ns, market_vol) in market_volumes {
        if *timestamp_ns < params.start_ns {
            continue;
        }
        if *timestamp_ns > params.end_ns {
            break;
        }

        let target_qty = ((*market_vol as f64) * params.target_pct).round() as u64;
        let qty = target_qty.min(remaining_qty);

        if qty > 0 {
            instructions.push(PovOrderInstruction {
                target_time_ns: *timestamp_ns,
                qty,
                market_volume: *market_vol,
            });

            remaining_qty = remaining_qty.saturating_sub(qty);
        }

        if remaining_qty == 0 {
            break;
        }
    }

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pov_basic() {
        let market_volumes = vec![
            (1000, 500),
            (2000, 500),
            (3000, 500),
        ];

        let params = PovParams {
            start_ns: 0,
            end_ns: 5000,
            total_qty: 240,
            target_pct: 0.2,
            num_slices: 10,
        };

        let schedule = compute_pov_schedule(params, &market_volumes);
        // 500 * 0.2 = 100 per slice, need 240 total = 3 slices (100 + 100 + 40)
        assert_eq!(schedule.len(), 3);

        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 240);
    }

    #[test]
    fn test_pov_respects_target_pct() {
        let market_volumes = vec![(1000, 1000)];

        let params = PovParams {
            start_ns: 0,
            end_ns: 5000,
            total_qty: 500,
            target_pct: 0.1,
            num_slices: 10,
        };

        let schedule = compute_pov_schedule(params, &market_volumes);
        assert_eq!(schedule[0].qty, 100);
    }
}
