"""Python helpers around the Rust execution-algorithm library.

- ``algo_exec_py._native``: PyO3 extension module built from ``crates/python-bindings``
- ``algo_exec_py.backtest``: a small execution-backtest harness built on it
"""

from ._native import (
    TICK_SCALE,
    compute_twap_py,
    compute_twap_randomized_py,
    price_from_float,
    price_to_float,
)

__version__ = "0.1.0"

__all__ = [
    "TICK_SCALE",
    "compute_twap_py",
    "compute_twap_randomized_py",
    "price_from_float",
    "price_to_float",
]
