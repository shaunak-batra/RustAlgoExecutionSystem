use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VwapError {
    #[error("Number of slices must be greater than zero")]
    InvalidSliceCount,

    #[error("Total quantity must be greater than zero")]
    InvalidQuantity,

    #[error("Historical data is empty or insufficient")]
    InsufficientData,

    #[error("Invalid time range: start={start}, end={end}")]
    InvalidTimeRange { start: u64, end: u64 },

    #[error("No historical data falls within execution window [{start}, {end}]")]
    NoDataInWindow { start: u64, end: u64 },
}

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
) -> Result<Vec<VwapOrderInstruction>, VwapError> {
    // Validate parameters
    if params.num_slices == 0 {
        return Err(VwapError::InvalidSliceCount);
    }

    if params.total_qty == 0 {
        return Err(VwapError::InvalidQuantity);
    }

    if historical_data.is_empty() {
        return Err(VwapError::InsufficientData);
    }

    let duration_ns = params.end_ns.saturating_sub(params.start_ns);
    if duration_ns == 0 || params.end_ns <= params.start_ns {
        return Err(VwapError::InvalidTimeRange {
            start: params.start_ns,
            end: params.end_ns,
        });
    }

    // Validate that at least some historical data falls within the execution window
    let has_data_in_window = historical_data
        .iter()
        .any(|(ts, _, _)| *ts >= params.start_ns && *ts < params.end_ns);

    if !has_data_in_window {
        return Err(VwapError::NoDataInWindow {
            start: params.start_ns,
            end: params.end_ns,
        });
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

    Ok(instructions)
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

        let schedule = compute_vwap_schedule(params, &historical_data).unwrap();
        assert_eq!(schedule.len(), 4);

        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 200);
    }

    #[test]
    fn test_vwap_distributes_evenly() {
        let historical_data = vec![(500, 100.0, 1000)];

        let params = VwapParams {
            start_ns: 0,
            end_ns: 1000,
            total_qty: 100,
            num_slices: 10,
        };

        let schedule = compute_vwap_schedule(params, &historical_data).unwrap();
        assert_eq!(schedule.len(), 10);

        for instruction in &schedule {
            assert_eq!(instruction.qty, 10);
        }
    }

    #[test]
    fn test_vwap_handles_remainder() {
        let historical_data = vec![(500, 100.0, 1000)];

        let params = VwapParams {
            start_ns: 0,
            end_ns: 1000,
            total_qty: 105,
            num_slices: 10,
        };

        let schedule = compute_vwap_schedule(params, &historical_data).unwrap();
        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 105);
    }
}
