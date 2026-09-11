use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct IsParams {
    pub start_ns: u64,
    pub end_ns: u64,
    pub total_qty: u64,
    pub decision_price: f64,
    pub urgency: f64,
    pub num_slices: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct IsOrderInstruction {
    pub target_time_ns: u64,
    pub qty: u64,
    pub max_participation_rate: f64,
}

pub fn compute_is_schedule(params: IsParams) -> Vec<IsOrderInstruction> {
    if params.num_slices == 0 || params.total_qty == 0 {
        return Vec::new();
    }

    let duration_ns = params.end_ns.saturating_sub(params.start_ns);
    if duration_ns == 0 {
        return Vec::new();
    }

    let urgency_clamped = params.urgency.clamp(0.0, 1.0);

    let mut instructions = Vec::new();
    let mut remaining_qty = params.total_qty;

    for i in 0..params.num_slices {
        if remaining_qty == 0 {
            break;
        }

        let t = i as f64 / params.num_slices as f64;

        let urgency_factor = 1.0 + (urgency_clamped * 2.0);
        let time_weight = 1.0 - (t * (1.0 - urgency_clamped));

        let participation_rate = (0.1 + (urgency_clamped * 0.3)) * time_weight;

        let target_fraction = if urgency_clamped > 0.7 {
            let exponential_weight = (urgency_factor * (1.0 - t)).exp();
            exponential_weight / urgency_factor.exp()
        } else {
            time_weight / params.num_slices as f64
        };

        let qty = ((params.total_qty as f64 * target_fraction).round() as u64).min(remaining_qty);

        if qty > 0 {
            let target_time_ns = params.start_ns
                + ((i as f64 / params.num_slices as f64) * duration_ns as f64) as u64;

            instructions.push(IsOrderInstruction {
                target_time_ns,
                qty,
                max_participation_rate: participation_rate.clamp(0.05, 0.5),
            });

            remaining_qty = remaining_qty.saturating_sub(qty);
        }
    }

    if remaining_qty > 0 && !instructions.is_empty() {
        if let Some(last) = instructions.last_mut() {
            last.qty += remaining_qty;
        }
    }

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_basic() {
        let params = IsParams {
            start_ns: 0,
            end_ns: 10000,
            total_qty: 1000,
            decision_price: 100.0,
            urgency: 0.5,
            num_slices: 10,
        };

        let schedule = compute_is_schedule(params);
        assert!(!schedule.is_empty());

        let total_qty: u64 = schedule.iter().map(|s| s.qty).sum();
        assert_eq!(total_qty, 1000);
    }

    #[test]
    fn test_is_high_urgency_front_loads() {
        let high_urgency_params = IsParams {
            start_ns: 0,
            end_ns: 10000,
            total_qty: 1000,
            decision_price: 100.0,
            urgency: 0.9,
            num_slices: 5,
        };

        let schedule = compute_is_schedule(high_urgency_params);

        if schedule.len() >= 2 {
            assert!(schedule[0].qty >= schedule[schedule.len() - 1].qty);
        }
    }

    #[test]
    fn test_is_participation_rate_bounds() {
        let params = IsParams {
            start_ns: 0,
            end_ns: 10000,
            total_qty: 1000,
            decision_price: 100.0,
            urgency: 0.5,
            num_slices: 10,
        };

        let schedule = compute_is_schedule(params);

        for instruction in &schedule {
            assert!(instruction.max_participation_rate >= 0.05);
            assert!(instruction.max_participation_rate <= 0.5);
        }
    }
}
