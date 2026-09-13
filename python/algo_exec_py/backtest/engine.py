"""Execution backtest harness: fills schedule child orders at prices from a price path."""

from bisect import bisect_left
from typing import Any, Dict, List, Optional, Sequence, Tuple

from algo_exec_py import twap_schedule, twap_schedule_randomized


class BacktestEngine:
    """Replays a TWAP schedule against a recorded price path.

    Each child order fills in full at the price linearly interpolated at its
    target time (clamped to the first or last observation outside the path).
    This is a no-impact, no-latency baseline: it measures how a schedule's
    timing interacts with price moves, not the market impact of trading.
    """

    def __init__(self, price_path: Sequence[Tuple[int, float]]):
        """
        Args:
            price_path: ``(timestamp_ns, price)`` observations, in any order.
                Timestamps must be unique: with two prices at one timestamp the
                interpolated price would depend on the order they were passed
                in. Aggregate such observations first.
        """
        if not price_path:
            raise ValueError("price_path must not be empty")
        self.price_path = sorted(price_path, key=lambda point: point[0])
        self._times = [time_ns for time_ns, _ in self.price_path]
        if len(set(self._times)) != len(self._times):
            raise ValueError("price_path timestamps must be unique")
        self.fills: List[Dict[str, Any]] = []
        self.side = "BUY"
        self.arrival_price: Optional[float] = None

    def run_twap(
        self,
        start_ns: int,
        end_ns: int,
        total_qty: int,
        num_slices: int,
        side: str = "BUY",
    ) -> List[Dict[str, Any]]:
        """Fill a TWAP schedule and return the fills."""
        schedule = twap_schedule(start_ns, end_ns, total_qty, num_slices)
        return self._fill(schedule, start_ns, side)

    def run_twap_randomized(
        self,
        start_ns: int,
        end_ns: int,
        total_qty: int,
        num_slices: int,
        seed: int = 42,
        side: str = "BUY",
    ) -> List[Dict[str, Any]]:
        """Fill a TWAP schedule whose release times are randomized within each slice."""
        schedule = twap_schedule_randomized(start_ns, end_ns, total_qty, num_slices, seed)
        return self._fill(schedule, start_ns, side)

    def price_at(self, time_ns: int) -> float:
        """Price linearly interpolated at ``time_ns``, clamped to the path's end points."""
        idx = bisect_left(self._times, time_ns)
        if idx == 0:
            return self.price_path[0][1]
        if idx == len(self.price_path):
            return self.price_path[-1][1]
        # _times[idx - 1] < time_ns <= _times[idx], so the denominator is positive.
        t0, p0 = self.price_path[idx - 1]
        t1, p1 = self.price_path[idx]
        return p0 + (time_ns - t0) / (t1 - t0) * (p1 - p0)

    def calculate_metrics(self) -> Dict[str, float]:
        """Execution quality of the last run.

        ``shortfall_bps`` is the cost against the arrival price (the price at the
        schedule start). It is positive when execution was worse than arrival:
        above it for a buy, below it for a sell.
        """
        if not self.fills or self.arrival_price is None:
            return {}

        total_qty = sum(fill["qty"] for fill in self.fills)
        total_notional = sum(fill["notional"] for fill in self.fills)
        vwap = total_notional / total_qty
        direction = 1.0 if self.side == "BUY" else -1.0
        shortfall_bps = (
            direction * (vwap - self.arrival_price) / self.arrival_price * 10_000
            if self.arrival_price > 0
            else float("nan")
        )

        return {
            "total_qty": total_qty,
            "total_notional": total_notional,
            "vwap": vwap,
            "arrival_price": self.arrival_price,
            "shortfall_bps": shortfall_bps,
            "num_fills": len(self.fills),
        }

    def _fill(
        self, schedule: List[Tuple[int, int]], start_ns: int, side: str
    ) -> List[Dict[str, Any]]:
        if side not in ("BUY", "SELL"):
            raise ValueError("side must be 'BUY' or 'SELL'")
        self.side = side
        self.arrival_price = self.price_at(start_ns)
        self.fills = []
        for time_ns, qty in schedule:
            price = self.price_at(time_ns)
            self.fills.append(
                {
                    "time_ns": time_ns,
                    "qty": qty,
                    "price": price,
                    "side": side,
                    "notional": qty * price,
                }
            )
        return self.fills


if __name__ == "__main__":
    SECOND = 1_000_000_000

    # Ten seconds of prices at 100 ms intervals, drifting up.
    path = [(i * SECOND // 10, 50_000.0 + i * 0.5) for i in range(100)]
    engine = BacktestEngine(path)

    engine.run_twap(0, 5 * SECOND, 1_000, 10, side="BUY")
    print("TWAP:", engine.calculate_metrics())

    engine.run_twap_randomized(0, 5 * SECOND, 1_000, 10, seed=42, side="BUY")
    print("Randomized TWAP:", engine.calculate_metrics())
