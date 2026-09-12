"""Python bindings for the Rust execution-algorithm library.

- ``algo_exec_py._native``: PyO3 extension built from ``crates/python-bindings``;
  its functions are re-exported here
- ``algo_exec_py.backtest``: a small execution-backtest harness built on them

Which exception a bad argument raises depends on how it is wrong: the wrong type
raises ``TypeError``, an integer outside its parameter's range (a negative
quantity, say) raises ``OverflowError``, and a value the scheduler rejects raises
``ValueError`` carrying the Rust error message.
"""

from ._native import (
    TICK_SCALE,
    almgren_chriss_schedule,
    pov_schedule,
    price_to_ticks,
    ticks_to_price,
    twap_schedule,
    twap_schedule_randomized,
    volume_profile_from_bars,
    vwap_schedule,
)

__version__ = "0.1.0"

__all__ = [
    "TICK_SCALE",
    "almgren_chriss_schedule",
    "pov_schedule",
    "price_to_ticks",
    "ticks_to_price",
    "twap_schedule",
    "twap_schedule_randomized",
    "volume_profile_from_bars",
    "vwap_schedule",
]
