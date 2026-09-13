"""
Tests for the Python bindings (``algo_exec_py._native``).

Where a definition is short enough to restate, results are compared with an
independent pure-Python implementation (integer TWAP rounding and slice starts,
the SplitMix64 draws of the randomized TWAP, the slice a volume bar falls in,
the Almgren-Chriss closed form). Otherwise they are checked against their
defining properties (the VWAP prefix bound, and the POV participation cap,
completeness and prefix invariance). Either way a mistake in the Rust code or
in the bindings shows up as a failure.

The import is unconditional on purpose: if the extension is not built, the
suite fails instead of silently skipping.
"""

import math
import sys

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


MASK64 = (1 << 64) - 1


def reference_splitmix64(seed):
    """The splitmix64.c generator used by ``twap_schedule_randomized``."""
    state = seed & MASK64
    while True:
        state = (state + 0x9E3779B97F4A7C15) & MASK64
        z = state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        yield (z ^ (z >> 31)) & MASK64


def reference_randomized_twap(start, end, total, n, seed):
    """Slice starts plus a multiply-shift draw per slice, including empty slices."""
    draws = reference_splitmix64(seed)
    bounds = reference_slice_starts(start, end, n) + [end]
    schedule = []
    for k, qty in enumerate(reference_twap_quantities(total, n)):
        offset = (next(draws) * (bounds[k + 1] - bounds[k])) >> 64
        if qty > 0:
            schedule.append((bounds[k] + offset, qty))
    return schedule


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

    def test_splitmix64_reference_matches_the_published_first_output(self):
        assert next(reference_splitmix64(0)) == 0xE220A8397B1DCDAF

    @pytest.mark.parametrize("total, n", [(5, 7), (100, 7), (1000, 10), (3, 64)])
    def test_randomized_matches_an_independent_generator(self, total, n):
        # 10_000 ns over 7 slices does not divide evenly, so the slice widths the
        # draws are scaled by differ.
        start, end, seed = 1_000, 11_000, 42
        assert ae.twap_schedule_randomized(start, end, total, n, seed) == (
            reference_randomized_twap(start, end, total, n, seed)
        )

    def test_randomized_slice_times_do_not_depend_on_the_quantity(self):
        # One draw per slice, so a slice that rounds to zero still consumes its
        # draw and the slices that do trade keep their times.
        sparse = ae.twap_schedule_randomized(1_000, 11_000, 5, 7, 42)
        dense = ae.twap_schedule_randomized(1_000, 11_000, 100, 7, 42)
        assert [time_ns for time_ns, _ in sparse] == [2_058, 4_254, 5_776, 6_768, 9_883]
        assert {t for t, _ in sparse} <= {t for t, _ in dense}

    @pytest.mark.parametrize(
        "args",
        [
            (0, 10, 0, 5, 1),  # zero quantity
            (0, 10, 100, 0, 1),  # zero slices
            (10, 10, 100, 5, 1),  # empty window
            (0, 4, 100, 5, 1),  # slices shorter than 1 ns
        ],
    )
    def test_randomized_invalid_input_raises(self, args):
        with pytest.raises(ValueError):
            ae.twap_schedule_randomized(*args)

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
            # Half a unit, plus the error of the profile's own float sums, which
            # grows with the slice count. A slack of 1e-9 * total would allow
            # 1000 units at the largest sizes and hide a real violation.
            slack = 0.5 + 4.0 * len(profile) * sys.float_info.epsilon * total
            assert abs(done - total * running / weight_total) <= slack

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

    def test_profile_slices_use_the_schedule_boundaries(self):
        # 10 ns in 3 slices start at 0, 3 and 6, as in twap_schedule and
        # vwap_schedule, so the bars at 3 and 6 open slices 1 and 2.
        bars = [(t, 1) for t in (2, 3, 5, 6, 9)]
        assert ae.volume_profile_from_bars(bars, 0, 10, 3, day_ns=100) == [0.2, 0.4, 0.4]

    @settings(max_examples=300, deadline=None)
    @given(duration=st.integers(50, 10**6), n=st.integers(1, 50), data=st.data())
    def test_a_bar_lands_in_the_slice_whose_bounds_contain_it(self, duration, n, data):
        offset = data.draw(st.integers(0, duration - 1))
        bounds = reference_slice_starts(0, duration, n) + [duration]
        expected = next(k for k in range(n) if bounds[k] <= offset < bounds[k + 1])
        profile = ae.volume_profile_from_bars([(offset, 1)], 0, duration, n, day_ns=duration)
        assert profile.index(1.0) == expected

    @pytest.mark.parametrize(
        "args, day_ns",
        [
            (([(0, 1)], 0, 10, 1), 0),  # zero-length day
            (([(0, 1)], 90, 20, 1), 100),  # window crosses midnight
            (([(50, 1)], 0, 10, 2), 100),  # no volume inside the window
            (([(0, 1)], 0, 10, 0), 100),  # zero slices
        ],
    )
    def test_profile_from_bars_invalid_input_raises(self, args, day_ns):
        with pytest.raises(ValueError):
            ae.volume_profile_from_bars(*args, day_ns=day_ns)


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
        # A gap of zero repeats a timestamp, which the schedule merges into one
        # child; small volumes produce increments that round to nothing.
        observations=st.lists(
            st.tuples(st.integers(0, 2), st.integers(0, 10**9)), max_size=40
        ),
    )
    def test_cap_completeness_and_prefix_invariance(self, bps, total, observations):
        data, timestamp = [], 0
        for gap, volume in observations:
            timestamp += gap
            data.append((timestamp, volume))
        full = ae.pov_schedule(0, 10**6, total, bps, data)
        children = full["children"]

        # Every child is a real order, and together they are scheduled_qty. Read
        # the list rather than a dict: a dict would hide a duplicate timestamp.
        assert all(qty > 0 for _, qty in children)
        assert [t for t, _ in children] == sorted({t for t, _ in children})
        assert sum(qty for _, qty in children) == full["scheduled_qty"]

        cumulative_volume, done, index = 0, 0, 0
        for i, (time_ns, volume) in enumerate(data):
            cumulative_volume += volume
            # Observations sharing a timestamp are one decision point.
            if i + 1 < len(data) and data[i + 1][0] == time_ns:
                continue
            if index < len(children) and children[index][0] == time_ns:
                done += children[index][1]
                index += 1
            assert done * 10_000 <= cumulative_volume * bps

        assert full["scheduled_qty"] == min(
            total, sum(v for _, v in data) * bps // 10_000
        )
        assert full["scheduled_qty"] + full["shortfall_qty"] == total

        # Once every observation at a timestamp has been seen, that timestamp's
        # child is settled and later data cannot change it.
        for cut in range(len(data) + 1):
            prefix = ae.pov_schedule(0, 10**6, total, bps, data[:cut])
            boundary = data[cut][0] if cut < len(data) else math.inf
            assert [c for c in prefix["children"] if c[0] < boundary] == [
                c for c in children if c[0] < boundary
            ]

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

    @pytest.mark.parametrize("total, n", [(100_000, 12), (9, 6), (15, 6), (3, 4), (1, 7)])
    def test_risk_neutral_schedule_is_twap(self, total, n):
        # 9 units over 6 slices is one of the sizes where rounding the float
        # trajectory gave 1,2,2,1,2,1 instead of TWAP's 2,1,2,1,2,1.
        params = dict(self.PARAMS, total_qty=total, num_slices=n)
        result = ae.almgren_chriss_schedule(risk_aversion=0.0, **params)
        assert result["children"] == ae.twap_schedule(
            params["start_ns"], params["end_ns"], total, n
        )

    def test_tiny_risk_aversion_keeps_kappa_positive(self):
        result = ae.almgren_chriss_schedule(risk_aversion=1e-20, **self.PARAMS)
        assert 0.0 < result["kappa"] < 1e-9

        # Why reference_almgren_chriss is not used here: at this size 1 + y is
        # exactly 1.0, so acosh(1 + y) collapses to zero. The Rust code keeps the
        # sqrt(2y) behaviour by computing ln1p(y + sqrt(y(y + 2))).
        tau = 3_600.0 / 12
        eta_tilde = 0.5 - 0.5 * 1e-6 * tau
        y = 0.5 * (1e-20 * 0.02**2 / eta_tilde) * tau * tau
        assert math.acosh(1.0 + y) == 0.0

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

    def test_native_module_version_matches_the_package(self):
        assert _native.__version__ == ae.__version__


class TestArgumentErrors:
    """Which exception a bad argument raises depends on how it is wrong.

    Arguments are converted to Rust types before any scheduler runs, so a value
    that cannot be converted never reaches the validation that raises
    ``ValueError``.
    """

    @pytest.mark.parametrize(
        "call",
        [
            lambda: ae.twap_schedule(0, SECOND, -5, 5),
            lambda: ae.twap_schedule(0, SECOND, 100, -1),
            lambda: ae.twap_schedule(0, SECOND, 2**64, 5),
            lambda: ae.pov_schedule(0, 10, 100, -1, [(1, 10)]),
            lambda: ae.pov_schedule(0, 10, 100, 100, [(1, -10)]),
        ],
    )
    def test_integers_outside_their_range_raise_overflow_error(self, call):
        with pytest.raises(OverflowError):
            call()

    @pytest.mark.parametrize(
        "call",
        [
            lambda: ae.twap_schedule(0, SECOND, 100.5, 5),
            lambda: ae.twap_schedule(0, SECOND, "100", 5),
            lambda: ae.pov_schedule(0, 10, 100, 1.5, [(1, 10)]),
            lambda: ae.price_to_ticks("x"),
            lambda: ae.vwap_schedule(0, SECOND, 100, ["a"]),
        ],
    )
    def test_wrong_types_raise_type_error(self, call):
        with pytest.raises(TypeError):
            call()

    def test_values_the_scheduler_rejects_raise_value_error(self):
        # Convertible and in range, so Rust validation is what rejects these.
        with pytest.raises(ValueError, match="participation_bps"):
            ae.pov_schedule(0, 10, 100, 0, [(1, 10)])
        with pytest.raises(ValueError, match="participation_bps"):
            ae.pov_schedule(0, 10, 100, 10_001, [(1, 10)])
        with pytest.raises(ValueError, match="quantity"):
            ae.twap_schedule(0, SECOND, 0, 5)


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

    def test_metrics_do_not_depend_on_the_input_order(self):
        def metrics(path):
            engine = BacktestEngine(path)
            engine.run_twap(0, 10 * SECOND, 1000, 10, side="BUY")
            return engine.calculate_metrics()

        assert metrics(list(reversed(self.PATH))) == metrics(self.PATH)

    def test_duplicate_timestamps_are_rejected(self):
        # Two prices at one timestamp would make the result depend on which of
        # them came first in the input.
        with pytest.raises(ValueError, match="unique"):
            BacktestEngine(self.PATH + [(5 * SECOND, 999.0)])

    def test_rejects_empty_path_and_unknown_side(self):
        with pytest.raises(ValueError):
            BacktestEngine([])
        with pytest.raises(ValueError):
            BacktestEngine(self.PATH).run_twap(0, 10 * SECOND, 100, 10, side="HOLD")


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
