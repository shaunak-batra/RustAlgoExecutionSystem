use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VwapParams {
    pub start_ns: u64,
    pub end_ns: u64,
    pub total_qty: u64,
    pub num_slices: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VwapOrderInstruction {
    pub target_time_ns: u64,
    pub qty: u64,
    pub expected_vwap: f64,
}

pub fn compute_vwap_schedule(
    params: VwapParams,
    historical_data: &[(u64, f64, u64)],
) -> Vec<VwapOrderInstruction> {
    if params.num_slices == 0 || params.total_qty == 0 || historical_data.is_empty() {
        return Vec::new();
    }

    let duration_ns = params.end_ns.saturating_sub(params.start_ns);
    if duration_ns == 0 {
        return Vec::new();
    }

    let slice_duration = duration_ns / params.num_slices as u64;

    let mut total_volume = 0u64;
    let mut total_pv = 0.0;

    for (_, price, volume) in historical_data {
        total_volume += volume;
        total_pv += price * (*volume as f64);
    }

    let overall_vwap = if total_volume > 0 {
        total_pv / total_volume as f64
    } else {
        0.0
    };

    let mut instructions = Vec::new();
    let base_qty = params.total_qty / params.num_slices as u64;
    let remainder = params.total_qty % params.num_slices as u64;

    for i in 0..params.num_slices {
        let target_time_ns = params.start_ns + (i as u64 * slice_duration);

        let qty = if i < remainder as usize {
            base_qty + 1
        } else {
            base_qty
        };

        if qty > 0 {
            let slice_start = target_time_ns;
            let slice_end = target_time_ns + slice_duration;

            let mut slice_volume = 0u64;
            let mut slice_pv = 0.0;

            for (ts, price, volume) in historical_data {
                if *ts >= slice_start && *ts < slice_end {
                    slice_volume += volume;
                    slice_pv += price * (*volume as f64);
                }
            }

            let expected_vwap = if slice_volume > 0 {
                slice_pv / slice_volume as f64
            } else {
                overall_vwap
            };

            instructions.push(VwapOrderInstruction {
                target_time_ns,
                qty,
                expected_vwap,
            });
        }
    }

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vwap_basic() {
        let historical_data = vec![
            (1000, 100.0, 1000),
            (2000, 101.0, 500),
            (3000, 99.0, 800),
        ];

        let params = VwapParams {
            start_ns: 0,
            end_ns: 4000,
            total_qty: 200,
            num_slices: 4,
        };

        let schedule = compute_vwap_schedule(params, &historical_data);
        assert_eq!(schedule.len(), 4);

        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 200);
    }

    #[test]
    fn test_vwap_distributes_evenly() {
        let historical_data = vec![(1000, 100.0, 1000)];

        let params = VwapParams {
            start_ns: 0,
            end_ns: 1000,
            total_qty: 100,
            num_slices: 10,
        };

        let schedule = compute_vwap_schedule(params, &historical_data);
        assert_eq!(schedule.len(), 10);

        for instruction in &schedule {
            assert_eq!(instruction.qty, 10);
        }
    }

    #[test]
    fn test_vwap_handles_remainder() {
        let historical_data = vec![(1000, 100.0, 1000)];

        let params = VwapParams {
            start_ns: 0,
            end_ns: 1000,
            total_qty: 105,
            num_slices: 10,
        };

        let schedule = compute_vwap_schedule(params, &historical_data);
        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 105);
    }
}
