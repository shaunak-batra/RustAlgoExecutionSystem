use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use algo_core::twap::{compute_twap_schedule, compute_twap_randomized, TwapParams};

/// Python wrapper for compute_twap_schedule.
/// Returns a list of tuples: [(target_time_ns, qty), ...]
#[pyfunction]
fn compute_twap_py(
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

    let schedule = compute_twap_schedule(params);

    Ok(schedule
        .into_iter()
        .map(|s| (s.target_time_ns, s.qty))
        .collect())
}

/// Python wrapper for compute_twap_randomized with jitter.
/// Returns a list of tuples: [(target_time_ns, qty), ...]
#[pyfunction]
fn compute_twap_randomized_py(
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

    let schedule = compute_twap_randomized(params, seed);

    Ok(schedule
        .into_iter()
        .map(|s| (s.target_time_ns, s.qty))
        .collect())
}

/// Price conversion utilities
#[pyfunction]
fn price_from_float(price: f64) -> i64 {
    (price * 100_000.0) as i64
}

#[pyfunction]
fn price_to_float(ticks: i64) -> f64 {
    ticks as f64 / 100_000.0
}

/// Python module definition
#[pymodule]
fn algo_exec_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(compute_twap_py, m)?)?;
    m.add_function(wrap_pyfunction!(compute_twap_randomized_py, m)?)?;
    m.add_function(wrap_pyfunction!(price_from_float, m)?)?;
    m.add_function(wrap_pyfunction!(price_to_float, m)?)?;

    // Add module-level constants
    m.add("__version__", "0.1.0")?;
    m.add("TICK_SCALE", 100_000)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twap_py_wrapper() {
        let schedule = compute_twap_py(0, 10_000, 100, 5).unwrap();
        assert_eq!(schedule.len(), 5);
        let total_qty: u64 = schedule.iter().map(|(_, q)| q).sum();
        assert_eq!(total_qty, 100);
    }

    #[test]
    fn test_price_conversions() {
        let ticks = price_from_float(42.50);
        assert_eq!(ticks, 4_250_000);
        let price = price_to_float(ticks);
        assert!((price - 42.50).abs() < 0.00001);
    }
}
