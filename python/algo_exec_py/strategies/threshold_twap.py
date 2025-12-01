"""
Threshold TWAP Strategy

This strategy demonstrates a simple algorithmic execution pattern:
- Monitor a price threshold
- When threshold is crossed, execute a TWAP order
- Track execution quality metrics
"""

import time
from typing import Optional
from algo_exec_py.client import ExecutionClient


class ThresholdTWAPStrategy:
    """
    Execute a TWAP when price crosses a threshold.

    Example use case:
    - Want to buy 10,000 shares of BTC-USD
    - Only execute if price drops below $48,000
    - Split execution over 5 minutes in 10 slices
    """

    def __init__(
        self,
        client: ExecutionClient,
        symbol: str,
        side: str,
        quantity: int,
        threshold_price: float,
        duration_sec: float,
        num_slices: int,
    ):
        """
        Initialize the strategy.

        Args:
            client: Execution client for order submission
            symbol: Trading symbol
            side: "BUY" or "SELL"
            quantity: Total quantity to execute
            threshold_price: Price threshold to trigger execution
            duration_sec: TWAP duration in seconds
            num_slices: Number of child orders
        """
        self.client = client
        self.symbol = symbol
        self.side = side
        self.quantity = quantity
        self.threshold_price = threshold_price
        self.duration_sec = duration_sec
        self.num_slices = num_slices

        self.parent_order_id: Optional[int] = None
        self.triggered = False

    def check_trigger(self, current_price: float) -> bool:
        """
        Check if price threshold is crossed.

        Args:
            current_price: Current market price

        Returns:
            True if triggered, False otherwise
        """
        if self.triggered:
            return False

        if self.side == "BUY" and current_price <= self.threshold_price:
            return True
        elif self.side == "SELL" and current_price >= self.threshold_price:
            return True

        return False

    def execute(self):
        """Execute the TWAP order."""
        if self.triggered:
            print(f"Strategy already triggered for {self.symbol}")
            return

        print(f"Executing {self.side} TWAP: {self.quantity} {self.symbol}")
        print(f"Duration: {self.duration_sec}s, Slices: {self.num_slices}")

        self.parent_order_id = self.client.submit_twap(
            symbol=self.symbol,
            side=self.side,
            quantity=self.quantity,
            duration_sec=self.duration_sec,
            num_slices=self.num_slices,
            limit_price=self.threshold_price,
        )

        self.triggered = True
        print(f"Order submitted: ID={self.parent_order_id}")

    def get_status(self):
        """Get current order status."""
        if not self.parent_order_id:
            return {"triggered": False}

        status = self.client.get_order_status(self.parent_order_id)
        return {
            "triggered": True,
            "parent_order_id": self.parent_order_id,
            "status": status["status"],
            "filled_qty": status["filled_qty"],
            "fill_pct": (status["filled_qty"] / self.quantity * 100) if self.quantity > 0 else 0,
        }

    def run(self, price_stream, poll_interval_sec: float = 1.0):
        """
        Run the strategy with a price stream.

        Args:
            price_stream: Iterator yielding (timestamp, price) tuples
            poll_interval_sec: Polling interval for status checks
        """
        print(f"Starting Threshold TWAP Strategy for {self.symbol}")
        print(f"Threshold: ${self.threshold_price:,.2f} ({self.side})")
        print(f"Waiting for trigger...\n")

        for timestamp, price in price_stream:
            if not self.triggered:
                print(f"[{timestamp}] Price: ${price:,.2f}", end="")

                if self.check_trigger(price):
                    print(" -> TRIGGERED!")
                    self.execute()
                else:
                    print()

            # Poll status if triggered
            if self.triggered:
                time.sleep(poll_interval_sec)
                status = self.get_status()
                print(f"[{timestamp}] Status: {status['status']}, "
                      f"Fill: {status['fill_pct']:.1f}%")

                if status["status"] in ["FILLED", "CANCELLED", "REJECTED"]:
                    print(f"\nStrategy completed: {status['status']}")
                    break


def simple_price_simulator(start_price: float, num_ticks: int, drift: float = 0.0):
    """
    Simple price simulator for testing.

    Args:
        start_price: Starting price
        num_ticks: Number of price updates
        drift: Drift per tick (can be negative)

    Yields:
        (timestamp_str, price) tuples
    """
    import datetime

    price = start_price
    for i in range(num_ticks):
        timestamp = datetime.datetime.now().strftime("%H:%M:%S")
        yield timestamp, price
        price += drift
        time.sleep(0.1)


# Example usage
if __name__ == "__main__":
    print("=== Threshold TWAP Strategy Demo ===\n")

    # Create client
    client = ExecutionClient("localhost:50051")
    client.connect()

    try:
        # Create strategy: Buy when price drops to $48,000
        strategy = ThresholdTWAPStrategy(
            client=client,
            symbol="BTC-USD",
            side="BUY",
            quantity=10_000,
            threshold_price=48_000.0,
            duration_sec=300.0,  # 5 minutes
            num_slices=10,
        )

        # Simulate price declining from $50,000 to $47,000
        price_stream = simple_price_simulator(
            start_price=50_000.0,
            num_ticks=50,
            drift=-100.0,  # Drop $100 per tick
        )

        # Run strategy
        strategy.run(price_stream, poll_interval_sec=1.0)

    finally:
        client.disconnect()

    print("\n=== Demo Complete ===")
