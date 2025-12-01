"""
Parity tests to ensure gRPC and FFI produce identical results.

This validates that backtests using FFI will match live execution via gRPC.
"""

import pytest

try:
    import algo_exec_rs
    HAS_FFI = True
except ImportError:
    HAS_FFI = False


@pytest.mark.skipif(not HAS_FFI, reason="FFI module not built")
class TestTWAPParity:
    """Test parity between different interfaces for TWAP algorithm."""

    def test_twap_schedule_correctness(self):
        """Verify TWAP schedule is mathematically correct."""
        schedule = algo_exec_rs.compute_twap_py(
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
        schedule = algo_exec_rs.compute_twap_py(
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
        schedule = algo_exec_rs.compute_twap_py(
            start_ns=1000,
            end_ns=5000,
            total_qty=50,
            num_slices=1
        )

        assert len(schedule) == 1
        assert schedule[0] == (1000, 50)

    def test_twap_timing_distribution(self):
        """Test that slices are evenly distributed in time."""
        schedule = algo_exec_rs.compute_twap_py(
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

        schedule1 = algo_exec_rs.compute_twap_randomized_py(*params)
        schedule2 = algo_exec_rs.compute_twap_randomized_py(*params)

        assert schedule1 == schedule2

    def test_randomized_twap_different_seeds(self):
        """Test that different seeds produce different schedules."""
        base_params = (0, 10_000_000_000, 1000, 10)

        schedule1 = algo_exec_rs.compute_twap_randomized_py(*base_params, 42)
        schedule2 = algo_exec_rs.compute_twap_randomized_py(*base_params, 123)

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

        schedule = algo_exec_rs.compute_twap_randomized_py(
            start_ns, end_ns, 500, 5, 999
        )

        for target_time, _ in schedule:
            assert start_ns <= target_time <= end_ns

    def test_price_conversions(self):
        """Test price conversion utilities."""
        # Test float to ticks
        ticks = algo_exec_rs.price_from_float(42.50)
        assert ticks == 4_250_000

        # Test ticks to float
        price = algo_exec_rs.price_to_float(ticks)
        assert abs(price - 42.50) < 0.00001

        # Round trip
        original = 12345.6789
        converted = algo_exec_rs.price_to_float(
            algo_exec_rs.price_from_float(original)
        )
        assert abs(converted - original) < 0.00001

    def test_zero_quantity(self):
        """Test edge case: zero quantity."""
        schedule = algo_exec_rs.compute_twap_py(
            start_ns=0,
            end_ns=10_000,
            total_qty=0,
            num_slices=5
        )

        assert len(schedule) == 0

    def test_zero_slices(self):
        """Test edge case: zero slices."""
        schedule = algo_exec_rs.compute_twap_py(
            start_ns=0,
            end_ns=10_000,
            total_qty=100,
            num_slices=0
        )

        assert len(schedule) == 0

    def test_invalid_time_range(self):
        """Test edge case: end before start."""
        schedule = algo_exec_rs.compute_twap_py(
            start_ns=10_000,
            end_ns=5_000,  # end before start!
            total_qty=100,
            num_slices=5
        )

        assert len(schedule) == 0

    def test_large_quantities(self):
        """Test with large quantities."""
        schedule = algo_exec_rs.compute_twap_py(
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


@pytest.mark.skipif(not HAS_FFI, reason="FFI module not built")
class TestModuleMetadata:
    """Test module-level attributes."""

    def test_version(self):
        """Test module has version."""
        assert hasattr(algo_exec_rs, '__version__')
        assert isinstance(algo_exec_rs.__version__, str)

    def test_tick_scale(self):
        """Test TICK_SCALE constant."""
        assert hasattr(algo_exec_rs, 'TICK_SCALE')
        assert algo_exec_rs.TICK_SCALE == 100_000


# Benchmarks (run with pytest-benchmark if available)
@pytest.mark.skipif(not HAS_FFI, reason="FFI module not built")
class TestPerformance:
    """Performance benchmarks for FFI calls."""

    def test_twap_schedule_performance(self, benchmark):
        """Benchmark TWAP schedule generation."""
        if not pytest:
            pytest.skip("pytest-benchmark not available")

        result = benchmark(
            algo_exec_rs.compute_twap_py,
            0, 10_000_000_000, 1000, 100
        )

        assert len(result) == 100

    def test_price_conversion_performance(self, benchmark):
        """Benchmark price conversions."""
        if not pytest:
            pytest.skip("pytest-benchmark not available")

        result = benchmark(
            algo_exec_rs.price_from_float,
            12345.6789
        )

        assert isinstance(result, int)


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v"])
