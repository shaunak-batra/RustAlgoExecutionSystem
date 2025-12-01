"""gRPC client for the execution engine."""

import time
from typing import Optional, Dict, Any
import grpc


class ExecutionClient:
    """Client for submitting orders to the execution engine via gRPC."""

    def __init__(self, address: str = "localhost:50051"):
        """
        Initialize the execution client.

        Args:
            address: The gRPC server address (host:port)
        """
        self.address = address
        self.channel = None
        self.stub = None

    def connect(self):
        """Establish connection to the gRPC server."""
        self.channel = grpc.insecure_channel(self.address)
        print(f"Connected to execution engine at {self.address}")

    def disconnect(self):
        """Close the gRPC connection."""
        if self.channel:
            self.channel.close()
            print("Disconnected from execution engine")

    def submit_twap(
        self,
        symbol: str,
        side: str,
        quantity: int,
        duration_sec: float,
        num_slices: int,
        limit_price: Optional[float] = None,
    ) -> int:
        """
        Submit a TWAP parent order.

        Args:
            symbol: Trading symbol (e.g., "BTC-USD")
            side: "BUY" or "SELL"
            quantity: Total quantity to execute
            duration_sec: Duration in seconds
            num_slices: Number of child orders
            limit_price: Optional limit price (None = market)

        Returns:
            parent_order_id: Unique ID for the parent order
        """
        if not self.channel:
            raise RuntimeError("Client not connected. Call connect() first.")

        start_ns = int(time.time() * 1e9)
        end_ns = start_ns + int(duration_sec * 1e9)

        limit_price_ticks = 0
        if limit_price is not None:
            limit_price_ticks = int(limit_price * 100_000)

        print(f"[STUB] Submitting TWAP: {side} {quantity} {symbol} over {duration_sec}s in {num_slices} slices")
        return 123456

    def get_order_status(self, parent_order_id: int) -> Dict[str, Any]:
        """
        Query the status of a parent order.

        Args:
            parent_order_id: The parent order ID

        Returns:
            Dictionary with order status information
        """
        if not self.channel:
            raise RuntimeError("Client not connected. Call connect() first.")

        return {
            "parent_order_id": parent_order_id,
            "status": "WORKING",
            "filled_qty": 0,
            "avg_fill_price": 0.0,
            "children": [],
        }

    def get_positions(self, symbol: Optional[str] = None) -> Dict[str, Any]:
        """
        Get current positions.

        Args:
            symbol: Optional symbol to filter (None = all positions)

        Returns:
            Dictionary with position information
        """
        if not self.channel:
            raise RuntimeError("Client not connected. Call connect() first.")

        return {
            "symbol": symbol or "ALL",
            "position": 0,
            "realized_pnl": 0.0,
            "unrealized_pnl": 0.0,
        }

    def __enter__(self):
        """Context manager entry."""
        self.connect()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit."""
        self.disconnect()


if __name__ == "__main__":
    # Example usage
    print("Testing Execution Client (stub mode)")

    with ExecutionClient() as client:
        # Submit a TWAP order
        order_id = client.submit_twap(
            symbol="BTC-USD",
            side="BUY",
            quantity=1000,
            duration_sec=10.0,
            num_slices=5,
            limit_price=50000.0,
        )
        print(f"Submitted order: {order_id}")

        # Query status
        status = client.get_order_status(order_id)
        print(f"Order status: {status}")

        # Get positions
        positions = client.get_positions("BTC-USD")
        print(f"Positions: {positions}")
