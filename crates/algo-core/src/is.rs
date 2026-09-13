//! Implementation shortfall (IS) schedules from the Almgren–Chriss model.
//!
//! Almgren, R. and Chriss, N. (2000). "Optimal execution of portfolio
//! transactions." *Journal of Risk* 3(2), 5–39.
//!
//! Trading `X` shares over `N` slices of length `τ` (horizon `T = Nτ`), with
//! holdings `x_0 = X, …, x_N = 0` and `n_k = x_{k−1} − x_k` traded in slice `k`:
//!
//! - permanent impact moves the price by `γ` per share traded;
//! - temporary impact costs `η · (n_k / τ)` per share traded in slice `k`;
//! - the unaffected price is a random walk with volatility `σ`.
//!
//! The implementation shortfall then has
//!
//! - expected value `E = ½·γ·X² + (η̃/τ)·Σ n_k²`, where `η̃ = η − ½·γ·τ`, and
//! - variance `V = σ²·τ·Σ_{k=1..N} x_k²`.
//!
//! Minimising `E + λ·V` gives `x_j = X · sinh(κ(T − t_j)) / sinh(κT)` with
//! `t_j = jτ` and `κ` solving `(2/τ²)(cosh(κτ) − 1) = λσ²/η̃`. With `λ = 0`
//! (risk-neutral) `κ` is zero, the trajectory is linear, and the schedule is
//! TWAP down to the same integer child quantities. Larger `λ` trades faster
//! early to cut exposure to price moves, at a higher expected impact cost.
//!
//! The fixed per-share cost of crossing the spread (the paper's `ε`) is left
//! out: it adds `ε·X` to the cost of every schedule that trades in one
//! direction, so it does not change the optimum. The schedule is the same for
//! buying and selling.
//!
//! Units must be consistent. With time in seconds: `σ` is in price units per
//! √second, `η` in price units per (share/second), `γ` in price units per
//! share, and `λ` in 1/currency.

use crate::schedule::{
    quantities_from_cumulative, slice_start, validate_window, ChildOrderInstruction, ScheduleError,
};
use serde::{Deserialize, Serialize};

const NANOS_PER_SECOND: f64 = 1e9;

/// Parameters for an Almgren–Chriss schedule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlmgrenChrissParams {
    /// Window start (nanoseconds).
    pub start_ns: u64,
    /// Window end, exclusive (nanoseconds).
    pub end_ns: u64,
    pub total_qty: u64,
    pub num_slices: usize,
    /// `λ ≥ 0`, in 1/currency. Zero gives exactly TWAP; see the module docs.
    pub risk_aversion: f64,
    /// `σ ≥ 0`: price volatility, in price units per √second.
    pub volatility: f64,
    /// `η > 0`: temporary impact, in price units per (share/second).
    pub temporary_impact: f64,
    /// `γ ≥ 0`: permanent impact, in price units per share.
    pub permanent_impact: f64,
}

/// Output of [`compute_almgren_chriss_schedule`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlmgrenChrissSchedule {
    /// Child orders at slice starts, quantities rounded from `holdings`.
    pub children: Vec<ChildOrderInstruction>,
    /// Optimal (unrounded) holdings `x_0 ..= x_N` at the slice boundaries.
    pub holdings: Vec<f64>,
    /// Urgency `κ`, in 1/second. When `κT` is large, holdings decay roughly like
    /// `e^{−κt}`. Zero when `λσ²` is zero or so small that `κ` underflows; the
    /// children are then exactly TWAP's.
    pub kappa: f64,
    /// Expected impact cost `E` of `holdings`, excluding the fixed spread cost.
    /// `+inf` if the parameters are extreme enough to overflow `f64`.
    pub expected_cost: f64,
    /// Variance `V` of the cost of `holdings`; `+inf` on overflow, as above.
    pub cost_variance: f64,
}

/// Computes the Almgren–Chriss optimal trajectory and rounds it into child
/// orders at slice starts, whose quantities sum exactly to `total_qty`. A slice
/// whose rounded quantity is zero is omitted, so with high urgency, or with
/// fewer units than slices, there are fewer children than slices.
///
/// # Examples
/// ```
/// use algo_core::is::{compute_almgren_chriss_schedule, AlmgrenChrissParams};
///
/// let params = AlmgrenChrissParams {
///     start_ns: 0,
///     end_ns: 3_600_000_000_000, // one hour
///     total_qty: 100_000,
///     num_slices: 12,
///     risk_aversion: 1e-3,
///     volatility: 0.02,
///     temporary_impact: 0.5,
///     permanent_impact: 1e-6,
/// };
/// let schedule = compute_almgren_chriss_schedule(params).unwrap();
///
/// // Risk aversion front-loads the schedule relative to TWAP's 8,333 per slice.
/// assert!(schedule.children[0].qty > 8_334);
/// assert_eq!(schedule.children.iter().map(|c| c.qty).sum::<u64>(), 100_000);
/// ```
pub fn compute_almgren_chriss_schedule(
    params: AlmgrenChrissParams,
) -> Result<AlmgrenChrissSchedule, ScheduleError> {
    let model = Model::new(&params)?;
    let kappa = model.kappa(&params)?;
    let n = params.num_slices;
    let total = params.total_qty as f64;
    let horizon = model.tau * n as f64;

    let mut holdings = Vec::with_capacity(n + 1);
    holdings.push(total);
    for j in 1..n {
        holdings.push(total * remaining_fraction(kappa, model.tau * j as f64, horizon));
    }
    holdings.push(0.0);

    // κ = 0 is exactly the linear trajectory, so round it the way TWAP does.
    // Rounding `1 − x/total` instead would agree mathematically but can fall on
    // the wrong side of an exact half, giving quantities TWAP would not produce.
    let quantities = if kappa == 0.0 {
        crate::twap::twap_quantities(params.total_qty, n).collect()
    } else {
        let cumulative: Vec<f64> = holdings[1..].iter().map(|&x| 1.0 - x / total).collect();
        quantities_from_cumulative(params.total_qty, &cumulative)
    };
    let children = quantities
        .into_iter()
        .enumerate()
        .filter(|&(_, qty)| qty > 0)
        .map(|(index, qty)| ChildOrderInstruction {
            target_time_ns: slice_start(params.start_ns, model.duration_ns, n, index),
            qty,
        })
        .collect();

    let (expected_cost, cost_variance) = model.cost(&params, &holdings);
    Ok(AlmgrenChrissSchedule {
        children,
        holdings,
        kappa,
        expected_cost,
        cost_variance,
    })
}

/// Expected cost `E` and variance `V` of any holdings trajectory under the
/// model, for comparing schedules (for example TWAP against the optimum).
///
/// `holdings` must have `num_slices + 1` entries, all finite, the first exactly
/// `total_qty as f64` and the last exactly zero. Either cost can be `+inf` if
/// the inputs are extreme enough to overflow `f64`.
pub fn almgren_chriss_cost(
    params: &AlmgrenChrissParams,
    holdings: &[f64],
) -> Result<(f64, f64), ScheduleError> {
    let model = Model::new(params)?;
    if holdings.len() != params.num_slices + 1 {
        return Err(ScheduleError::InvalidParameter {
            name: "holdings",
            reason: "must have num_slices + 1 entries",
        });
    }
    if holdings.first() != Some(&(params.total_qty as f64)) || holdings.last() != Some(&0.0) {
        return Err(ScheduleError::InvalidParameter {
            name: "holdings",
            reason: "must start at total_qty and end at zero",
        });
    }
    if holdings.iter().any(|x| !x.is_finite()) {
        return Err(ScheduleError::InvalidParameter {
            name: "holdings",
            reason: "must all be finite",
        });
    }
    Ok(model.cost(params, holdings))
}

/// Validated quantities shared by the schedule and cost functions.
struct Model {
    duration_ns: u64,
    /// Slice length in seconds.
    tau: f64,
    /// `η̃ = η − ½γτ`.
    eta_tilde: f64,
}

impl Model {
    fn new(params: &AlmgrenChrissParams) -> Result<Self, ScheduleError> {
        let duration_ns = validate_window(
            params.start_ns,
            params.end_ns,
            params.total_qty,
            params.num_slices,
        )?;
        require_non_negative("risk_aversion", params.risk_aversion)?;
        require_non_negative("volatility", params.volatility)?;
        require_non_negative("permanent_impact", params.permanent_impact)?;
        if !params.temporary_impact.is_finite() || params.temporary_impact <= 0.0 {
            return Err(ScheduleError::InvalidParameter {
                name: "temporary_impact",
                reason: "must be finite and positive",
            });
        }

        let tau = duration_ns as f64 / NANOS_PER_SECOND / params.num_slices as f64;
        let eta_tilde = params.temporary_impact - 0.5 * params.permanent_impact * tau;
        if eta_tilde.is_nan() || eta_tilde <= 0.0 {
            return Err(ScheduleError::InvalidParameter {
                name: "temporary_impact",
                reason: "must exceed permanent_impact * slice_seconds / 2 (the model needs η̃ > 0)",
            });
        }
        Ok(Self {
            duration_ns,
            tau,
            eta_tilde,
        })
    }

    /// `κ` from `(2/τ²)(cosh(κτ) − 1) = λσ²/η̃`.
    fn kappa(&self, params: &AlmgrenChrissParams) -> Result<f64, ScheduleError> {
        let kappa_tilde_sq =
            params.risk_aversion * params.volatility * params.volatility / self.eta_tilde;
        // κτ = acosh(1 + y) with y = κ̃²τ²/2, written as ln(1 + y + √(y(y+2)))
        // via ln_1p so it stays accurate when y is tiny.
        let y = 0.5 * kappa_tilde_sq * self.tau * self.tau;
        // √(y(y+2)): for large y the product would overflow even though κ is
        // perfectly finite, so factor y out; at or below 1 the direct form
        // cannot overflow and is accurate to rounding.
        let root = if y > 1.0 {
            y * (1.0 + 2.0 / y).sqrt()
        } else {
            (y * (y + 2.0)).sqrt()
        };
        let kappa = (y + root).ln_1p() / self.tau;
        // κ itself is at most about ln(2y)/τ, so it can only be non-finite
        // because λσ²/η̃ or y overflowed on the way.
        if kappa.is_finite() {
            Ok(kappa)
        } else {
            Err(ScheduleError::InvalidParameter {
                name: "risk_aversion",
                reason:
                    "risk_aversion * volatility^2 / η̃ is too large for f64, so κ cannot be computed",
            })
        }
    }

    /// `E = ½γX² + (η̃/τ)·Σ n_k²` and `V = σ²τ·Σ_{k≥1} x_k²`.
    fn cost(&self, params: &AlmgrenChrissParams, holdings: &[f64]) -> (f64, f64) {
        let total = params.total_qty as f64;
        let squared_trades: f64 = holdings.windows(2).map(|w| (w[0] - w[1]).powi(2)).sum();
        let expected = 0.5 * params.permanent_impact * total * total
            + self.eta_tilde / self.tau * squared_trades;
        // Scaling each holding before squaring keeps a zero trajectory at zero
        // variance. Computing σ² first would give inf · 0 = NaN for a huge σ.
        let variance = self.tau
            * holdings[1..]
                .iter()
                .map(|x| (params.volatility * x).powi(2))
                .sum::<f64>();
        (expected, variance)
    }
}

fn require_non_negative(name: &'static str, value: f64) -> Result<(), ScheduleError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(ScheduleError::InvalidParameter {
            name,
            reason: "must be finite and non-negative",
        })
    }
}

/// `sinh(κ(T − t)) / sinh(κT)`, evaluated without overflow when `κT` is large.
fn remaining_fraction(kappa: f64, t: f64, horizon: f64) -> f64 {
    // sinh(a)/sinh(b) = e^(a−b) · (1 − e^(−2a)) / (1 − e^(−2b)), with a = κ(T − t), b = κT.
    let denominator = (-2.0 * kappa * horizon).exp_m1();
    if denominator == 0.0 {
        // κT is zero or too small to register: sinh is linear, which is TWAP.
        return (horizon - t) / horizon;
    }
    let numerator = (-2.0 * kappa * (horizon - t)).exp_m1();
    (-kappa * t).exp() * numerator / denominator
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twap::{compute_twap_schedule, TwapParams};
    use proptest::prelude::*;

    const HOUR_NS: u64 = 3_600_000_000_000;
    const TOTAL: u64 = 100_000;
    const SLICES: usize = 12;
    const SIGMA: f64 = 0.02;
    const ETA: f64 = 0.5;
    const GAMMA: f64 = 1e-6;

    fn params(risk_aversion: f64) -> AlmgrenChrissParams {
        AlmgrenChrissParams {
            start_ns: 0,
            end_ns: HOUR_NS,
            total_qty: TOTAL,
            num_slices: SLICES,
            risk_aversion,
            volatility: SIGMA,
            temporary_impact: ETA,
            permanent_impact: GAMMA,
        }
    }

    /// Slice length in seconds and `η̃` for [`params`].
    fn tau_and_eta_tilde() -> (f64, f64) {
        let tau = 3_600.0 / SLICES as f64;
        (tau, ETA - 0.5 * GAMMA * tau)
    }

    fn linear_holdings() -> Vec<f64> {
        (0..=SLICES)
            .map(|j| TOTAL as f64 * (1.0 - j as f64 / SLICES as f64))
            .collect()
    }

    fn objective(p: &AlmgrenChrissParams, holdings: &[f64]) -> f64 {
        let (expected, variance) = almgren_chriss_cost(p, holdings).unwrap();
        expected + p.risk_aversion * variance
    }

    #[test]
    fn risk_neutral_schedule_is_twap() {
        let schedule = compute_almgren_chriss_schedule(params(0.0)).unwrap();
        assert_eq!(schedule.kappa, 0.0);
        for (x, expected) in schedule.holdings.iter().zip(linear_holdings()) {
            assert!((x - expected).abs() < 1e-6, "{x} vs {expected}");
        }

        // The children equal TWAP's for every size, not just sizes that divide
        // evenly. Rounding the float trajectory instead agrees mathematically but
        // lands on the wrong side of an exact half for sizes like 9 over 6.
        for num_slices in 1..40 {
            for total_qty in (1u64..200).chain([999, 1_000, 100_000, (1u64 << 53) + 7]) {
                let p = AlmgrenChrissParams {
                    total_qty,
                    num_slices,
                    ..params(0.0)
                };
                let is = compute_almgren_chriss_schedule(p).unwrap();
                let twap = compute_twap_schedule(TwapParams {
                    start_ns: p.start_ns,
                    end_ns: p.end_ns,
                    total_qty,
                    num_slices,
                })
                .unwrap();
                assert_eq!(
                    is.children, twap,
                    "{total_qty} units over {num_slices} slices"
                );
            }
        }
    }

    #[test]
    fn kappa_solves_the_discrete_urgency_equation() {
        let (tau, eta_tilde) = tau_and_eta_tilde();
        for lambda in [1e-6, 1e-4, 1e-3, 1e-2] {
            let kappa = compute_almgren_chriss_schedule(params(lambda))
                .unwrap()
                .kappa;
            let lhs = 2.0 / (tau * tau) * ((kappa * tau).cosh() - 1.0);
            let rhs = lambda * SIGMA * SIGMA / eta_tilde;
            assert!(
                (lhs - rhs).abs() <= 1e-9 * rhs,
                "lambda {lambda}: {lhs} vs {rhs}"
            );
        }
    }

    #[test]
    fn holdings_match_the_textbook_sinh_formula() {
        let (tau, _) = tau_and_eta_tilde();
        let horizon = tau * SLICES as f64;
        // 1e-11 puts κT near 3e-4, where the curve is very nearly linear: close
        // enough that a loose tolerance would accept a straight line, and far
        // enough that this one does not.
        for lambda in [1e-11, 1e-8, 1e-6, 1e-3, 1e-1] {
            let schedule = compute_almgren_chriss_schedule(params(lambda)).unwrap();
            let kappa = schedule.kappa;
            for (j, &x) in schedule.holdings.iter().enumerate() {
                let t = tau * j as f64;
                let expected =
                    TOTAL as f64 * (kappa * (horizon - t)).sinh() / (kappa * horizon).sinh();
                assert!(
                    (x - expected).abs() <= 1e-9 * TOTAL as f64,
                    "lambda {lambda}, j = {j}: {x} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn holdings_satisfy_the_first_order_optimality_condition() {
        // d(E + λV)/dx_k = 0 at every interior k, i.e.
        // (η̃/τ²)(x_{k−1} − 2x_k + x_{k+1}) = λσ² x_k.
        let (tau, eta_tilde) = tau_and_eta_tilde();
        let scale = eta_tilde / (tau * tau) * TOTAL as f64;
        for lambda in [1e-4, 1e-3, 1e-2] {
            let x = compute_almgren_chriss_schedule(params(lambda))
                .unwrap()
                .holdings;
            for k in 1..SLICES {
                let residual = eta_tilde / (tau * tau) * (x[k - 1] - 2.0 * x[k] + x[k + 1])
                    - lambda * SIGMA * SIGMA * x[k];
                assert!(
                    residual.abs() <= 1e-9 * scale,
                    "lambda {lambda}, k {k}: residual {residual}"
                );
            }
        }
    }

    #[test]
    fn risk_averse_schedule_trades_cost_for_lower_variance() {
        let p = params(1e-3);
        let schedule = compute_almgren_chriss_schedule(p).unwrap();
        let twap = linear_holdings();
        let (twap_cost, twap_variance) = almgren_chriss_cost(&p, &twap).unwrap();

        assert!(objective(&p, &schedule.holdings) < objective(&p, &twap));
        assert!(schedule.expected_cost > twap_cost);
        assert!(schedule.cost_variance < twap_variance);
    }

    #[test]
    fn more_risk_aversion_trades_faster() {
        let schedules: Vec<_> = [0.0, 1e-4, 1e-3, 1e-2]
            .iter()
            .map(|&lambda| compute_almgren_chriss_schedule(params(lambda)).unwrap())
            .collect();
        for pair in schedules.windows(2) {
            let (slow, fast) = (&pair[0], &pair[1]);
            assert!(fast.kappa > slow.kappa);
            for (x_slow, x_fast) in slow.holdings.iter().zip(&fast.holdings) {
                assert!(*x_fast <= x_slow + 1e-9);
            }
            assert!(fast.children[0].qty >= slow.children[0].qty);
        }
    }

    #[test]
    fn cost_formulas_match_closed_form_for_twap() {
        let p = params(0.0);
        let (tau, eta_tilde) = tau_and_eta_tilde();
        let (x, n) = (TOTAL as f64, SLICES as f64);
        let (expected, variance) = almgren_chriss_cost(&p, &linear_holdings()).unwrap();

        let expected_closed = 0.5 * GAMMA * x * x + eta_tilde * x * x / (n * tau);
        // Σ_{k=1..N} (1 − k/N)² = (N − 1)·N·(2N − 1) / (6N²)
        let variance_closed =
            SIGMA * SIGMA * tau * x * x * (n - 1.0) * n * (2.0 * n - 1.0) / (6.0 * n * n);
        assert!((expected - expected_closed).abs() <= 1e-12 * expected_closed);
        assert!((variance - variance_closed).abs() <= 1e-12 * variance_closed);
    }

    #[test]
    fn single_slice_trades_everything_at_the_start() {
        let mut p = params(1e-3);
        p.num_slices = 1;
        let schedule = compute_almgren_chriss_schedule(p).unwrap();
        assert_eq!(
            schedule.children,
            vec![ChildOrderInstruction {
                target_time_ns: 0,
                qty: TOTAL
            }]
        );
        assert_eq!(schedule.holdings, vec![TOTAL as f64, 0.0]);
        assert_eq!(schedule.cost_variance, 0.0);
    }

    #[test]
    fn extreme_urgency_is_numerically_stable() {
        let urgent = compute_almgren_chriss_schedule(params(1e6)).unwrap();
        assert!(urgent.kappa.is_finite());
        assert!(urgent.holdings.iter().all(|x| x.is_finite()));
        assert!(urgent.holdings.windows(2).all(|w| w[1] <= w[0]));
        assert!(urgent.children[0].qty >= TOTAL * 99 / 100);
        assert_eq!(urgent.children.iter().map(|c| c.qty).sum::<u64>(), TOTAL);

        let nearly_neutral = compute_almgren_chriss_schedule(params(1e-300)).unwrap();
        for (x, expected) in nearly_neutral.holdings.iter().zip(linear_holdings()) {
            assert!((x - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn extreme_risk_aversion_stays_finite() {
        // y = λσ²τ²/(2η̃) reaches 3.6e201 at λ = 1e200. Squaring it to form
        // √(y(y+2)) would overflow and reject a κ that is really about 1.55, and
        // at λ = 1e30 the horizon reaches κT ≈ 880, where evaluating the sinh
        // ratio directly gives inf/inf = NaN.
        for lambda in [1e30, 1e200] {
            let schedule = compute_almgren_chriss_schedule(params(lambda)).unwrap();
            assert!(
                schedule.kappa.is_finite() && schedule.kappa > 0.0,
                "lambda {lambda}: kappa {}",
                schedule.kappa
            );
            assert!(schedule.holdings.iter().all(|x| x.is_finite()));
            assert!(schedule.holdings.windows(2).all(|w| w[1] <= w[0]));
            assert_eq!(schedule.children.iter().map(|c| c.qty).sum::<u64>(), TOTAL);
            // At this urgency the whole order goes in the first slice.
            assert_eq!(schedule.children[0].qty, TOTAL);
        }
    }

    #[test]
    fn tiny_risk_aversion_keeps_kappa_positive() {
        // Here y is about 3.6e-19, so `1 + y` is exactly 1.0 in f64 and
        // acosh(1 + y) would collapse κ to zero. Writing κτ as
        // ln_1p(y + √(y(y+2))) keeps the √(2y) behaviour instead.
        let schedule = compute_almgren_chriss_schedule(params(1e-20)).unwrap();
        assert!(schedule.kappa > 0.0, "kappa {}", schedule.kappa);
        assert!(schedule.kappa < 1e-9, "kappa {}", schedule.kappa);
    }

    #[test]
    fn an_eta_tilde_of_exactly_zero_is_rejected() {
        // τ = 2 s exactly, so η̃ = η − γτ/2 = 0.25 − 0.25 = 0 with no rounding.
        // The model divides by η̃, so zero has to be rejected, not just negatives.
        let p = AlmgrenChrissParams {
            end_ns: 2_000_000_000,
            num_slices: 1,
            temporary_impact: 0.25,
            permanent_impact: 0.25,
            ..params(1e-3)
        };
        assert!(matches!(
            compute_almgren_chriss_schedule(p),
            Err(ScheduleError::InvalidParameter {
                name: "temporary_impact",
                ..
            })
        ));
    }

    #[test]
    fn variance_is_zero_rather_than_nan_when_nothing_is_held() {
        // σ² overflows to infinity, and one slice holds nothing after it trades,
        // so forming σ² · τ · Σx² would give inf · 0 = NaN.
        let p = AlmgrenChrissParams {
            volatility: f64::MAX,
            num_slices: 1,
            ..params(0.0)
        };
        let schedule = compute_almgren_chriss_schedule(p).unwrap();
        assert_eq!(schedule.cost_variance, 0.0);
        assert!(schedule.expected_cost.is_finite());
    }

    #[test]
    fn a_slightly_wrong_urgency_costs_more() {
        // The trajectory is optimal in κ too, not only against arbitrary
        // perturbations: rebuilding it with κ off by 5% is measurably worse.
        let p = params(1e-3);
        let optimal = compute_almgren_chriss_schedule(p).unwrap();
        let best = objective(&p, &optimal.holdings);
        let (tau, _) = tau_and_eta_tilde();
        let horizon = tau * SLICES as f64;

        for factor in [0.95, 1.05] {
            let kappa = optimal.kappa * factor;
            let holdings: Vec<f64> = (0..=SLICES)
                .map(|j| {
                    let t = tau * j as f64;
                    TOTAL as f64 * (kappa * (horizon - t)).sinh() / (kappa * horizon).sinh()
                })
                .collect();
            assert!(
                objective(&p, &holdings) > best,
                "kappa x {factor} should cost more than the optimum"
            );
        }
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        let rejects = |p: AlmgrenChrissParams, field: &str| {
            assert!(
                matches!(
                    compute_almgren_chriss_schedule(p),
                    Err(ScheduleError::InvalidParameter { name, .. }) if name == field
                ),
                "expected {field} to be rejected"
            );
        };

        rejects(params(-1.0), "risk_aversion");
        rejects(
            AlmgrenChrissParams {
                volatility: f64::NAN,
                ..params(1e-3)
            },
            "volatility",
        );
        rejects(
            AlmgrenChrissParams {
                temporary_impact: 0.0,
                ..params(1e-3)
            },
            "temporary_impact",
        );
        rejects(
            AlmgrenChrissParams {
                permanent_impact: -1e-9,
                ..params(1e-3)
            },
            "permanent_impact",
        );
        // γτ/2 = 0.01 · 300 / 2 = 1.5 exceeds η = 0.5, so η̃ would be negative.
        rejects(
            AlmgrenChrissParams {
                permanent_impact: 0.01,
                ..params(1e-3)
            },
            "temporary_impact",
        );
        rejects(
            AlmgrenChrissParams {
                volatility: 1e10,
                ..params(f64::MAX)
            },
            "risk_aversion",
        );
        assert_eq!(
            compute_almgren_chriss_schedule(AlmgrenChrissParams {
                total_qty: 0,
                ..params(1e-3)
            }),
            Err(ScheduleError::ZeroQuantity)
        );
    }

    #[test]
    fn cost_rejects_malformed_holdings() {
        let p = params(1e-3);
        assert!(almgren_chriss_cost(&p, &[TOTAL as f64, 0.0]).is_err());
        let mut not_liquidated = linear_holdings();
        not_liquidated[SLICES] = 1.0;
        assert!(almgren_chriss_cost(&p, &not_liquidated).is_err());

        // Non-finite holdings would silently produce a NaN cost.
        for bad in [f64::INFINITY, f64::NAN] {
            let mut holdings = linear_holdings();
            holdings[1] = bad;
            assert!(almgren_chriss_cost(&p, &holdings).is_err(), "{bad}");
        }
    }

    proptest! {
        #[test]
        fn holdings_minimise_expected_cost_plus_risk(
            // Small perturbations. Large ones make the quadratic term dominate, so
            // the objective rises whatever trajectory it started from, and the
            // test would pass for a clearly non-optimal schedule.
            perturbation in prop::collection::vec(-20.0f64..20.0, SLICES - 1),
        ) {
            let p = params(1e-3);
            let optimal = compute_almgren_chriss_schedule(p).unwrap().holdings;
            let best = objective(&p, &optimal);
            let mut other = optimal.clone();
            for (x, delta) in other[1..SLICES].iter_mut().zip(&perturbation) {
                *x += delta;
            }
            prop_assert!(objective(&p, &other) >= best - 1e-9 * best.abs());
        }

        #[test]
        fn schedule_invariants(
            total_qty in 1u64..1_000_000_000,
            num_slices in 1usize..200,
            extra_ns in 0u64..10_000_000_000_000,
            risk_aversion in 0.0f64..1e-2,
            volatility in 0.0f64..0.1,
            temporary_impact in 1e-3f64..10.0,
            permanent_impact in 0.0f64..1e-8,
        ) {
            let p = AlmgrenChrissParams {
                start_ns: 5,
                end_ns: 5 + num_slices as u64 + extra_ns,
                total_qty,
                num_slices,
                risk_aversion,
                volatility,
                temporary_impact,
                permanent_impact,
            };
            let s = compute_almgren_chriss_schedule(p).unwrap();

            prop_assert_eq!(s.children.iter().map(|c| c.qty).sum::<u64>(), total_qty);
            prop_assert!(s.children.windows(2).all(|w| w[0].target_time_ns < w[1].target_time_ns));
            prop_assert!(s.children.iter().all(|c| c.qty > 0 && (p.start_ns..p.end_ns).contains(&c.target_time_ns)));
            prop_assert_eq!(s.holdings.len(), num_slices + 1);
            prop_assert!(s.holdings.iter().all(|x| x.is_finite() && *x >= 0.0));
            prop_assert!(s.holdings.windows(2).all(|w| w[1] <= w[0] * (1.0 + 1e-12) + 1e-9));
            prop_assert!(s.kappa.is_finite() && s.kappa >= 0.0);
            prop_assert!(s.expected_cost.is_finite() && s.expected_cost >= 0.0);
            prop_assert!(s.cost_variance.is_finite() && s.cost_variance >= 0.0);
        }
    }
}
