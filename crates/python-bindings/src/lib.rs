//! Python bindings for the execution algorithms, installed as `algo_exec_py._native`.
//!
//! Arguments are converted to Rust types before any scheduler runs, so which
//! exception a caller sees depends on how the input is wrong: the wrong type
//! raises `TypeError`, an integer outside its parameter's range (a negative
//! quantity, say) raises `OverflowError`, and a value the scheduler itself
//! rejects raises `ValueError` carrying the Rust error message.

// `#[pyfunction]` in pyo3 0.22 expands to a `PyErr -> PyErr` conversion that clippy flags.
#![allow(clippy::useless_conversion)]

use algo_core::{
    compute_almgren_chriss_schedule, compute_pov_schedule, compute_twap_randomized,
    compute_twap_schedule, compute_vwap_schedule, AlmgrenChrissParams, ChildOrderInstruction,
    IntradayWindow, PovParams, TwapParams, VwapParams,
};
use orderbook::Price;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn value_error(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn as_pairs(children: &[ChildOrderInstruction]) -> Vec<(u64, u64)> {
    children
        .iter()
        .map(|child| (child.target_time_ns, child.qty))
        .collect()
}

/// TWAP schedule as a list of `(target_time_ns, qty)` tuples.
#[pyfunction]
fn twap_schedule(
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    num_slices: usize,
) -> PyResult<Vec<(u64, u64)>> {
    let params = TwapParams {
        start_ns,
        end_ns,
        total_qty,
        num_slices,
    };
    compute_twap_schedule(params)
        .map(|schedule| as_pairs(&schedule))
        .map_err(value_error)
}

/// TWAP schedule with each child released at a seeded random time inside its slice.
#[pyfunction]
fn twap_schedule_randomized(
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    num_slices: usize,
    seed: u64,
) -> PyResult<Vec<(u64, u64)>> {
    let params = TwapParams {
        start_ns,
        end_ns,
        total_qty,
        num_slices,
    };
    compute_twap_randomized(params, seed)
        .map(|schedule| as_pairs(&schedule))
        .map_err(value_error)
}

/// VWAP schedule following `volume_profile`, one non-negative weight per slice.
#[pyfunction]
fn vwap_schedule(
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    volume_profile: Vec<f64>,
) -> PyResult<Vec<(u64, u64)>> {
    let params = VwapParams {
        start_ns,
        end_ns,
        total_qty,
    };
    compute_vwap_schedule(params, &volume_profile)
        .map(|schedule| as_pairs(&schedule))
        .map_err(value_error)
}

/// Intraday volume profile from `(timestamp_ns, volume)` bars: one fraction per
/// slice, summing to 1 up to floating-point rounding.
#[pyfunction]
#[pyo3(signature = (bars, start_offset_ns, duration_ns, num_slices, day_ns = IntradayWindow::NANOS_PER_DAY))]
fn volume_profile_from_bars(
    bars: Vec<(u64, u64)>,
    start_offset_ns: u64,
    duration_ns: u64,
    num_slices: usize,
    day_ns: u64,
) -> PyResult<Vec<f64>> {
    let window = IntradayWindow {
        start_offset_ns,
        duration_ns,
        day_ns,
    };
    algo_core::volume_profile_from_bars(&bars, window, num_slices).map_err(value_error)
}

/// POV schedule as a dict with `children`, `scheduled_qty` and `shortfall_qty`.
#[pyfunction]
fn pov_schedule<'py>(
    py: Python<'py>,
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    participation_bps: u32,
    market_volume: Vec<(u64, u64)>,
) -> PyResult<Bound<'py, PyDict>> {
    let params = PovParams {
        start_ns,
        end_ns,
        total_qty,
        participation_bps,
    };
    let schedule = compute_pov_schedule(params, &market_volume).map_err(value_error)?;
    let result = PyDict::new_bound(py);
    result.set_item("children", as_pairs(&schedule.children))?;
    result.set_item("scheduled_qty", schedule.scheduled_qty)?;
    result.set_item("shortfall_qty", schedule.shortfall_qty)?;
    Ok(result)
}

/// Almgren–Chriss implementation-shortfall schedule as a dict with `children`,
/// `holdings`, `kappa`, `expected_cost` and `cost_variance`.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn almgren_chriss_schedule<'py>(
    py: Python<'py>,
    start_ns: u64,
    end_ns: u64,
    total_qty: u64,
    num_slices: usize,
    risk_aversion: f64,
    volatility: f64,
    temporary_impact: f64,
    permanent_impact: f64,
) -> PyResult<Bound<'py, PyDict>> {
    let params = AlmgrenChrissParams {
        start_ns,
        end_ns,
        total_qty,
        num_slices,
        risk_aversion,
        volatility,
        temporary_impact,
        permanent_impact,
    };
    let schedule = compute_almgren_chriss_schedule(params).map_err(value_error)?;
    let result = PyDict::new_bound(py);
    result.set_item("children", as_pairs(&schedule.children))?;
    result.set_item("holdings", schedule.holdings)?;
    result.set_item("kappa", schedule.kappa)?;
    result.set_item("expected_cost", schedule.expected_cost)?;
    result.set_item("cost_variance", schedule.cost_variance)?;
    Ok(result)
}

/// Converts a price to integer ticks, rounding `price * TICK_SCALE` half away
/// from zero. Exact for prices with at most five decimals below 2^35 (about
/// 3.44e10) in magnitude; see `Price::from_f64`.
#[pyfunction]
fn price_to_ticks(price: f64) -> PyResult<i64> {
    Price::try_from_f64(price)
        .map(|price| price.ticks())
        .map_err(value_error)
}

/// Converts integer ticks to a price.
#[pyfunction]
fn ticks_to_price(ticks: i64) -> f64 {
    Price::new(ticks).as_f64()
}

/// Python module definition, installed as `algo_exec_py._native`.
#[pymodule]
#[pyo3(name = "_native")]
fn algo_exec_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(twap_schedule, m)?)?;
    m.add_function(wrap_pyfunction!(twap_schedule_randomized, m)?)?;
    m.add_function(wrap_pyfunction!(vwap_schedule, m)?)?;
    m.add_function(wrap_pyfunction!(volume_profile_from_bars, m)?)?;
    m.add_function(wrap_pyfunction!(pov_schedule, m)?)?;
    m.add_function(wrap_pyfunction!(almgren_chriss_schedule, m)?)?;
    m.add_function(wrap_pyfunction!(price_to_ticks, m)?)?;
    m.add_function(wrap_pyfunction!(ticks_to_price, m)?)?;

    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("TICK_SCALE", Price::TICK_SCALE as i64)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twap_wrapper_returns_time_quantity_pairs() {
        assert_eq!(
            twap_schedule(0, 10_000, 100, 5).unwrap(),
            vec![(0, 20), (2_000, 20), (4_000, 20), (6_000, 20), (8_000, 20)]
        );
    }

    #[test]
    fn price_conversions_round_to_the_nearest_tick() {
        assert_eq!(price_to_ticks(0.29).unwrap(), 29_000);
        assert_eq!(ticks_to_price(4_250_000), 42.5);
    }
}
