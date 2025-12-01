"""Algorithmic Execution Engine - Python Package

This package provides:
- gRPC client for interacting with the Rust execution engine
- FFI bindings for backtesting via PyO3
- Backtest harness and analysis tools
"""

__version__ = "0.1.0"

# FFI module will be imported when available after maturin build
try:
    import algo_exec_rs
    __all__ = ["algo_exec_rs"]
except ImportError:
    # Not yet built with maturin
    pass
