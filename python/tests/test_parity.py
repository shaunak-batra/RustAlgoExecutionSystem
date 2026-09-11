"""
Tests for the Python bindings (``algo_exec_py._native``).

Results are compared with independent pure-Python implementations of the
same definitions (integer TWAP rounding, the Almgren-Chriss closed form, the
POV participation cap), so a mistake in the Rust code or in the bindings shows
up as a mismatch.

The import is unconditional on purpose: if the extension is not built, the
suite fails instead of silently skipping.
"""

import math

import pytest
from hypothesis import given, settings, strategies as st

import algo_exec_py as ae
from algo_exec_py import _native
from algo_exec_py.backtest.engine import BacktestEngine

SECOND = 1_000_000_000


def reference_twap_quantities(total, n):
    """Cumulative target total*k/n rounded half up, differenced into slices."""
    done, quantities = 0, []
    for k in range(1, n + 1):
        target = (2 * total * k + n) // (2 * n)
        quantities.append(target - done)
        done = target
    return quantities


def reference_slice_starts(start, end, n):
    return [start + (end - start) * k // n for k in range(n)]


class TestTwap:
    def test_even_split_and_timing(self):
        assert ae.twap_schedule(0, 10 * SECOND, 1000, 10) == [
            (k * SECOND, 100) for k in range(10)
        ]

    def test_remainder_is_spread_not_front_loaded(self):
        schedule = ae.twap_schedule(0, 5 * SECOND, 103, 5)
        assert [qty for _, qty in schedule] == [21, 20, 21, 20, 21]

    def test_fewer_units_than_slices_are_spaced_out(self):
        assert ae.twap_schedule(0, 10 * SECOND, 5, 10) == [
            (k * SECOND, 1) for k in (0, 2, 4, 6, 8)
        ]

    @settings(max_examples=300, deadline=None)
    @given(
        start=st.integers(0, 10**12),
        extra=st.integers(0, 10**12),
        total=st.integers(1, 2**64 - 1),
        n=st.integers(1, 400),
    )
    def test_matches_reference_and_sums_exactly(self, start, extra, total, n):
        end = start + n + extra
        expected = [
            (time_ns, qty)
            for time_ns, qty in zip(
                reference_slice_starts(start, end, n), reference_twap_quantities(total, n)
            )
            if qty > 0
        ]
        schedule = ae.twap_schedule(start, end, total, n)
        assert schedule == expected
        assert sum(qty for _, qty in schedule) == total

    @pytest.mark.parametrize(
        "args",
        [
            (0, 10, 0, 5),  # zero quantity
            (0, 10, 100, 0),  # zero slices
            (10, 10, 100, 5),  # empty window
            (10, 5, 100, 5),  # end before start
            (0, 4, 100, 5),  # slices shorter than 1 ns
        ],
    )
    def test_invalid_input_raises(self, args):
        with pytest.raises(ValueError):
            ae.twap_schedule(*args)

    def test_randomized_times_stay_in_their_slices(self):
        start, end, n = SECOND, 11 * SECOND, 10
        schedule = ae.twap_schedule_randomized(start, end, 1000, n, 42)
        assert schedule == ae.twap_schedule_randomized(start, end, 1000, n, 42)
        assert schedule != ae.twap_schedule_randomized(start, end, 1000, n, 43)
        bounds = reference_slice_starts(start, end, n) + [end]
        for k, (time_ns, qty) in enumerate(schedule):
            assert bounds[k] <= time_ns < bounds[k + 1]
            assert qty == 100


class TestVwap:
    def test_follows_volume_profile(self):
        assert ae.vwap_schedule(0, 2 * SECOND, 1000, [9.0, 1.0]) == [(0, 900), (SECOND, 100)]

    def test_zero_volume_slices_get_no_orders(self):
        assert ae.vwap_schedule(0, 4 * SECOND, 100, [1.0, 0.0, 1.0, 0.0]) == [
            (0, 50),
            (2 * SECOND, 50),
        ]

    @settings(max_examples=200, deadline=None)
    @given(
        total=st.integers(1, 10**12),
        profile=st.lists(
            st.floats(0, 1e6, allow_nan=False, allow_infinity=False), min_size=1, max_size=100
        ).filter(lambda p: sum(p) > 0),
    )
    def test_every_prefix_is_within_half_a_unit_of_the_profile(self, total, profile):
        schedule = ae.vwap_schedule(0, len(profile) * SECOND, total, profile)
        assert sum(qty for _, qty in schedule) == total

        by_slice = {time_ns // SECOND: qty for time_ns, qty in schedule}
        weight_total = sum(profile)
        done, running = 0, 0.0
        for k, weight in enumerate(profile):
            if weight == 0:
                assert k not in by_slice
            done += by_slice.get(k, 0)
            running += weight
            assert abs(done - total * running / weight_total) <= 0.5 + 1e-9 * total

    @pytest.mark.parametrize(
        "profile", [[], [0.0, 0.0], [1.0, -1.0], [1.0, float("nan")], [float("inf")]]
    )
    def test_invalid_profile_raises(self, profile):
        with pytest.raises(ValueError):
            ae.vwap_schedule(0, 10 * SECOND, 100, profile)

    def test_profile_from_bars_pools_days_by_time_of_day(self):
        day = 86_400 * SECOND
        open_ = 9 * 3_600 * SECOND
        bars = [
            (open_, 30),
            (open_ + 1_800 * SECOND, 10),
            (day + open_ + 60 * SECOND, 50),
            (day + open_ + 3_000 * SECOND, 10),
            (day + 20 * 3_600 * SECOND, 1_000),  # outside the window
        ]
        profile = ae.volume_profile_from_bars(bars, open_, 3_600 * SECOND, 2)
        assert profile == pytest.approx([0.8, 0.2])


class TestPov:
    def test_participation_is_rounded_down_and_shortfall_reported(self):
        result = ae.pov_schedule(0, 10, 1000, 1000, [(1, 105), (2, 105), (3, 90)])
        assert result["children"] == [(1, 10), (2, 11), (3, 9)]
        assert result["scheduled_qty"] == 30
        assert result["shortfall_qty"] == 970

    @settings(max_examples=150, deadline=None)
    @given(
        bps=st.integers(1, 10_000),
        total=st.integers(1, 10**9),
        volumes=st.lists(st.integers(0, 10**9), max_size=40),
    )
    def test_cap_completeness_and_causality(self, bps, total, volumes):
        data = list(enumerate(volumes))
        full = ae.pov_schedule(0, 10**6, total, bps, data)
        children = dict(full["children"])

        cumulative_volume, done = 0, 0
        for time_ns, volume in data:
            cumulative_volume += volume
            done += children.get(time_ns, 0)
            assert done * 10_000 <= cumulative_volume * bps

        assert full["scheduled_qty"] == min(total, sum(volumes) * bps // 10_000)
        assert full["scheduled_qty"] + full["shortfall_qty"] == total

        for cut in range(len(data) + 1):
            prefix = ae.pov_schedule(0, 10**6, total, bps, data[:cut])
            assert prefix["children"] == [c for c in full["children"] if c[0] < cut]

    @pytest.mark.parametrize(
        "bps, data",
        [(0, [(1, 10)]), (10_001, [(1, 10)]), (100, [(2, 10), (1, 10)])],
    )
    def test_invalid_input_raises(self, bps, data):
        with pytest.raises(ValueError):
            ae.pov_schedule(0, 10, 100, bps, data)


def reference_almgren_chriss(total, n, horizon_s, lam, sigma, eta, gamma):
    """Holdings and kappa straight from the paper's formulas, using math.sinh/acosh."""
    tau = horizon_s / n
    eta_tilde = eta - 0.5 * gamma * tau
    kappa = math.acosh(1 + 0.5 * (lam * sigma**2 / eta_tilde) * tau * tau) / tau
    if kappa == 0:
        return [total * (1 - j / n) for j in range(n + 1)], kappa
    horizon = n * tau
    holdings = [
        total * math.sinh(kappa * (horizon - j * tau)) / math.sinh(kappa * horizon)
        for j in range(n + 1)
    ]
    return holdings, kappa


class TestAlmgrenChriss:
    PARAMS = dict(
        start_ns=0,
        end_ns=3_600 * SECOND,
        total_qty=100_000,
        num_slices=12,
        volatility=0.02,
        temporary_impact=0.5,
        permanent_impact=1e-6,
    )

    @pytest.mark.parametrize("lam", [0.0, 1e-4, 1e-3, 1e-2])
    def test_matches_independent_closed_form(self, lam):
        result = ae.almgren_chriss_schedule(risk_aversion=lam, **self.PARAMS)
        holdings, kappa = reference_almgren_chriss(100_000, 12, 3_600.0, lam, 0.02, 0.5, 1e-6)
        assert result["kappa"] == pytest.approx(kappa, rel=1e-9, abs=1e-15)
        assert result["holdings"] == pytest.approx(holdings, rel=1e-9, abs=1e-6)
        assert sum(qty for _, qty in result["children"]) == 100_000

    def test_risk_neutral_schedule_is_twap(self):
        result = ae.almgren_chriss_schedule(risk_aversion=0.0, **self.PARAMS)
        assert result["children"] == ae.twap_schedule(0, 3_600 * SECOND, 100_000, 12)

    def test_cost_and_variance_formulas(self):
        result = ae.almgren_chriss_schedule(risk_aversion=1e-3, **self.PARAMS)
        x = result["holdings"]
        tau = 300.0
        eta_tilde = 0.5 - 0.5 * 1e-6 * tau
        expected_cost = 0.5 * 1e-6 * 100_000**2 + eta_tilde / tau * sum(
            (a - b) ** 2 for a, b in zip(x, x[1:])
        )
        variance = 0.02**2 * tau * sum(v * v for v in x[1:])
        assert result["expected_cost"] == pytest.approx(expected_cost, rel=1e-12)
        assert result["cost_variance"] == pytest.approx(variance, rel=1e-12)

    @pytest.mark.parametrize(
        "override",
        [
            {"risk_aversion": -1.0},
            {"temporary_impact": 0.0},
            {"permanent_impact": 0.01},  # makes eta_tilde negative
            {"volatility": float("nan")},
        ],
    )
    def test_invalid_parameters_raise(self, override):
        kwargs = dict(self.PARAMS, risk_aversion=1e-3)
        kwargs.update(override)
        with pytest.raises(ValueError):
            ae.almgren_chriss_schedule(**kwargs)


class TestPricesAndMetadata:
    def test_prices_round_to_the_nearest_tick(self):
        assert ae.price_to_ticks(0.29) == 29_000
        assert ae.price_to_ticks(42.5) == 4_250_000
        assert ae.ticks_to_price(4_250_000) == 42.5
        assert ae.TICK_SCALE == 100_000

    @pytest.mark.parametrize("bad", [float("nan"), float("inf"), 1e300])
    def test_invalid_prices_raise(self, bad):
        with pytest.raises(ValueError):
            ae.price_to_ticks(bad)

    def test_native_module_version(self):
        assert isinstance(_native.__version__, str)


class TestBacktestHarness:
    PATH = [(i * SECOND, 100.0 + i) for i in range(11)]  # +1.0 per second

    def test_buy_shortfall_against_arrival(self):
        engine = BacktestEngine(self.PATH)
        fills = engine.run_twap(0, 10 * SECOND, 1000, 10, side="BUY")
        assert len(fills) == 10
        metrics = engine.calculate_metrics()
        # Equal quantities at 100..109 -> VWAP 104.5, 4.5% above the 100 arrival price.
        assert metrics["vwap"] == pytest.approx(104.5)
        assert metrics["arrival_price"] == 100.0
        assert metrics["shortfall_bps"] == pytest.approx(450.0)

    def test_sell_into_a_rising_market_has_negative_shortfall(self):
        engine = BacktestEngine(self.PATH)
        engine.run_twap(0, 10 * SECOND, 1000, 10, side="SELL")
        assert engine.calculate_metrics()["shortfall_bps"] == pytest.approx(-450.0)

    def test_prices_are_interpolated_and_clamped(self):
        engine = BacktestEngine(self.PATH)
        assert engine.price_at(SECOND // 2) == pytest.approx(100.5)
        assert engine.price_at(-5) == 100.0
        assert engine.price_at(99 * SECOND) == 110.0

    def test_rejects_empty_path_and_unknown_side(self):
        with pytest.raises(ValueError):
            BacktestEngine([])
        with pytest.raises(ValueError):
            BacktestEngine(self.PATH).run_twap(0, 10 * SECOND, 100, 10, side="HOLD")


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
