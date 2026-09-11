"""
Tests for the PyO3 bindings (``algo_exec_py._native``).

They check that the Python-visible functions return exactly what the Rust
library computes (same slice counts, quantities, and timestamps).

The import is unconditional on purpose: if the extension is not built, the
suite fails instead of silently skipping.
"""

import pytest

import algo_exec_py as ae
from algo_exec_py import _native


class TestTWAPBindings:
    """TWAP schedule computed through the bindings."""

    def test_twap_schedule_correctness(self):
        """Verify TWAP schedule is mathematically correct."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=10_000_000_000,  # 10 seconds
            total_qty=1000,
            num_slices=10
        )

        # Check number of slices
        assert len(schedule) == 10

        # Check total quantity
        total_qty = sum(qty for _, qty in schedule)
        assert total_qty == 1000

        # Check each slice has correct quantity
        for _, qty in schedule:
            assert qty == 100

        # Check timing
        assert schedule[0][0] == 0
        assert schedule[-1][0] == 9_000_000_000

    def test_twap_with_remainder(self):
        """Test TWAP with quantity that doesn't divide evenly."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=5_000_000_000,
            total_qty=103,
            num_slices=5
        )

        assert len(schedule) == 5
        total_qty = sum(qty for _, qty in schedule)
        assert total_qty == 103

        # First 3 slices should get 21, last 2 get 20
        assert schedule[0][1] == 21
        assert schedule[1][1] == 21
        assert schedule[2][1] == 21
        assert schedule[3][1] == 20
        assert schedule[4][1] == 20

    def test_twap_single_slice(self):
        """Test TWAP with single slice (degenerate case)."""
        schedule = ae.compute_twap_py(
            start_ns=1000,
            end_ns=5000,
            total_qty=50,
            num_slices=1
        )

        assert len(schedule) == 1
        assert schedule[0] == (1000, 50)

    def test_twap_timing_distribution(self):
        """Test that slices are evenly distributed in time."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=10_000_000_000,
            total_qty=100,
            num_slices=10
        )

        # Check time intervals are equal
        expected_interval = 1_000_000_000  # 1 second
        for i in range(len(schedule) - 1):
            interval = schedule[i + 1][0] - schedule[i][0]
            assert interval == expected_interval

    def test_randomized_twap_determinism(self):
        """Test that randomized TWAP is deterministic with same seed."""
        params = (0, 10_000_000_000, 1000, 10, 42)

        schedule1 = ae.compute_twap_randomized_py(*params)
        schedule2 = ae.compute_twap_randomized_py(*params)

        assert schedule1 == schedule2

    def test_randomized_twap_different_seeds(self):
        """Test that different seeds produce different schedules."""
        base_params = (0, 10_000_000_000, 1000, 10)

        schedule1 = ae.compute_twap_randomized_py(*base_params, 42)
        schedule2 = ae.compute_twap_randomized_py(*base_params, 123)

        # Quantities should be same
        qtys1 = [qty for _, qty in schedule1]
        qtys2 = [qty for _, qty in schedule2]
        assert qtys1 == qtys2

        # But timings should differ
        times1 = [t for t, _ in schedule1]
        times2 = [t for t, _ in schedule2]
        assert times1 != times2

    def test_randomized_twap_bounds(self):
        """Test that randomized times stay within start/end bounds."""
        start_ns = 1_000_000_000
        end_ns = 11_000_000_000

        schedule = ae.compute_twap_randomized_py(
            start_ns, end_ns, 500, 5, 999
        )

        for target_time, _ in schedule:
            assert start_ns <= target_time <= end_ns

    def test_price_conversions(self):
        """Test price conversion utilities."""
        # Test float to ticks
        ticks = ae.price_from_float(42.50)
        assert ticks == 4_250_000

        # Test ticks to float
        price = ae.price_to_float(ticks)
        assert abs(price - 42.50) < 0.00001

        # Round trip
        original = 12345.6789
        converted = ae.price_to_float(ae.price_from_float(original))
        assert abs(converted - original) < 0.00001

    def test_zero_quantity(self):
        """Test edge case: zero quantity."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=10_000,
            total_qty=0,
            num_slices=5
        )

        assert len(schedule) == 0

    def test_zero_slices(self):
        """Test edge case: zero slices."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=10_000,
            total_qty=100,
            num_slices=0
        )

        assert len(schedule) == 0

    def test_invalid_time_range(self):
        """Test edge case: end before start."""
        schedule = ae.compute_twap_py(
            start_ns=10_000,
            end_ns=5_000,  # end before start!
            total_qty=100,
            num_slices=5
        )

        assert len(schedule) == 0

    def test_large_quantities(self):
        """Test with large quantities."""
        schedule = ae.compute_twap_py(
            start_ns=0,
            end_ns=1_000_000_000,
            total_qty=1_000_000,
            num_slices=100
        )

        assert len(schedule) == 100
        total_qty = sum(qty for _, qty in schedule)
        assert total_qty == 1_000_000

        # Each slice should be 10,000
        for _, qty in schedule:
            assert qty == 10_000


class TestModuleMetadata:
    """Test module-level attributes."""

    def test_version(self):
        """Test native module has version."""
        assert isinstance(_native.__version__, str)

    def test_tick_scale(self):
        """Test TICK_SCALE constant."""
        assert ae.TICK_SCALE == 100_000


class TestBacktestHarness:
    """The pure-Python harness runs end to end on top of the bindings."""

    def test_twap_backtest_vwap_matches_linear_path(self):
        from algo_exec_py.backtest.engine import BacktestEngine

        # Price rises linearly by 1.0 per second; TWAP fills at slice starts 0..9s.
        path = [(i * 1_000_000_000, 100.0 + i) for i in range(11)]
        engine = BacktestEngine(path)
        fills = engine.run_twap(0, 10_000_000_000, 1000, 10)

        assert len(fills) == 10
        metrics = engine.calculate_metrics()
        assert metrics["total_qty"] == 1000
        # Equal quantities at prices 100..109 -> VWAP 104.5
        assert metrics["vwap"] == pytest.approx(104.5)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
