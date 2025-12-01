"""Backtest engine using Rust FFI bindings for TWAP execution."""

from typing import List, Tuple, Dict, Any
import bisect

# Import the Rust module (will be available after maturin build)
try:
    import algo_exec_rs
    HAS_FFI = True
except ImportError:
    HAS_FFI = False
    print("Warning: algo_exec_rs not found. Run 'maturin develop' to build the Rust extension.")


class BacktestEngine:
    """Backtest engine that simulates TWAP execution using FFI."""

    def __init__(self, price_path: List[Tuple[int, float]]):
        """
        Initialize the backtest engine.

        Args:
            price_path: List of (timestamp_ns, price) tuples representing market prices
        """
        if not HAS_FFI:
            raise RuntimeError("Rust FFI module not available. Build with maturin first.")

        self.price_path = sorted(price_path, key=lambda x: x[0])
        self.fills: List[Dict[str, Any]] = []
        self.slippage_bps: List[float] = []

    def run_twap(
        self,
        start_ns: int,
        end_ns: int,
        total_qty: int,
        num_slices: int,
        side: str = "BUY",
    ) -> List[Dict[str, Any]]:
        """
        Run a TWAP execution simulation.

        Args:
            start_ns: Start time in nanoseconds
            end_ns: End time in nanoseconds
            total_qty: Total quantity to execute
            num_slices: Number of child orders
            side: "BUY" or "SELL"

        Returns:
            List of fill dictionaries
        """
        # Compute TWAP schedule using Rust FFI
        schedule = algo_exec_rs.compute_twap_py(start_ns, end_ns, total_qty, num_slices)

        self.fills = []

        for target_time_ns, qty in schedule:
            # Simulate fill at target time using interpolated price
            fill_price = self._interpolate_price(target_time_ns)

            if fill_price is None:
                print(f"Warning: No price data available at time {target_time_ns}")
                continue

            fill = {
                "time_ns": target_time_ns,
                "qty": qty,
                "price": fill_price,
                "side": side,
                "notional": qty * fill_price,
            }

            self.fills.append(fill)

        return self.fills

    def run_twap_randomized(
        self,
        start_ns: int,
        end_ns: int,
        total_qty: int,
        num_slices: int,
        seed: int = 42,
        side: str = "BUY",
    ) -> List[Dict[str, Any]]:
        """
        Run a TWAP execution with randomized timing (jitter).

        Args:
            start_ns: Start time in nanoseconds
            end_ns: End time in nanoseconds
            total_qty: Total quantity to execute
            num_slices: Number of child orders
            seed: Random seed for reproducibility
            side: "BUY" or "SELL"

        Returns:
            List of fill dictionaries
        """
        # Compute randomized TWAP schedule using Rust FFI
        schedule = algo_exec_rs.compute_twap_randomized_py(
            start_ns, end_ns, total_qty, num_slices, seed
        )

        self.fills = []

        for target_time_ns, qty in schedule:
            fill_price = self._interpolate_price(target_time_ns)

            if fill_price is None:
                continue

            fill = {
                "time_ns": target_time_ns,
                "qty": qty,
                "price": fill_price,
                "side": side,
                "notional": qty * fill_price,
            }

            self.fills.append(fill)

        return self.fills

    def _interpolate_price(self, target_ns: int) -> float | None:
        """
        Interpolate price at target time from price path.

        Args:
            target_ns: Target timestamp in nanoseconds

        Returns:
            Interpolated price or None if out of bounds
        """
        if not self.price_path:
            return None

        # Binary search for surrounding points
        idx = bisect.bisect_left(self.price_path, (target_ns, 0.0))

        # Handle edge cases
        if idx == 0:
            return self.price_path[0][1]
        if idx >= len(self.price_path):
            return self.price_path[-1][1]

        # Linear interpolation
        t0, p0 = self.price_path[idx - 1]
        t1, p1 = self.price_path[idx]

        if t1 == t0:
            return p0

        alpha = (target_ns - t0) / (t1 - t0)
        return p0 + alpha * (p1 - p0)

    def calculate_metrics(self) -> Dict[str, float]:
        """
        Calculate execution quality metrics.

        Returns:
            Dictionary with metrics like VWAP, total notional, etc.
        """
        if not self.fills:
            return {}

        total_qty = sum(f["qty"] for f in self.fills)
        total_notional = sum(f["notional"] for f in self.fills)
        vwap = total_notional / total_qty if total_qty > 0 else 0.0

        arrival_price = self.price_path[0][1] if self.price_path else 0.0
        slippage_bps = ((vwap - arrival_price) / arrival_price * 10_000) if arrival_price > 0 else 0.0

        return {
            "total_qty": total_qty,
            "total_notional": total_notional,
            "vwap": vwap,
            "arrival_price": arrival_price,
            "slippage_bps": slippage_bps,
            "num_fills": len(self.fills),
        }


# Example usage
if __name__ == "__main__":
    import time

    print("Backtest Engine Example")

    if not HAS_FFI:
        print("ERROR: Rust FFI module not available.")
        print("Run: cd python && maturin develop --release")
        exit(1)

    # Generate synthetic price path (10 seconds of data at 100ms intervals)
    start_time = int(time.time() * 1e9)
    price_path = [
        (start_time + i * 100_000_000, 50000.0 + i * 0.5)  # Slow drift upward
        for i in range(100)
    ]

    # Create backtest engine
    engine = BacktestEngine(price_path)

    # Run TWAP execution
    print("\n=== Standard TWAP ===")
    fills = engine.run_twap(
        start_ns=start_time,
        end_ns=start_time + 5_000_000_000,  # 5 seconds
        total_qty=1000,
        num_slices=10,
        side="BUY",
    )

    print(f"Generated {len(fills)} fills")
    for fill in fills[:3]:
        print(f"  Time: {fill['time_ns']}, Qty: {fill['qty']}, Price: {fill['price']:.2f}")

    metrics = engine.calculate_metrics()
    print(f"\nMetrics:")
    for key, value in metrics.items():
        print(f"  {key}: {value:.4f}")

    # Run randomized TWAP
    print("\n=== Randomized TWAP ===")
    fills_rand = engine.run_twap_randomized(
        start_ns=start_time,
        end_ns=start_time + 5_000_000_000,
        total_qty=1000,
        num_slices=10,
        seed=42,
        side="BUY",
    )

    print(f"Generated {len(fills_rand)} fills with jitter")
    metrics_rand = engine.calculate_metrics()
    print(f"Slippage: {metrics_rand['slippage_bps']:.2f} bps")
