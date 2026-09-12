# Algorithmic execution engine

A limit order book, four execution-scheduling algorithms, and a deterministic
execution engine that drives them, written in Rust. A gRPC API exposes the
engine; a desktop client and Python bindings sit on top.

The point of the project is execution quality rather than strategy: given a
parent order to fill over a window, how should it be broken into child orders,
and what did that cost against the price when the decision was made?

Everything below is either a property checked by a test in this repository or a
number measured by a benchmark in it. Where a component is simulated or a
feature absent, that is stated rather than implied.

- Repository: <https://github.com/shaunak-batra/RustAlgoExecutionSystem>
- Built and tested with stable Rust (rustc 1.91.1), edition 2021. No minimum
  supported version is claimed, because none has been tested.

---

## Contents

- [What this is, and what it is not](#what-this-is-and-what-it-is-not)
- [Build and run](#build-and-run)
- [Architecture](#architecture)
- [The order book](#the-order-book)
- [The scheduling algorithms](#the-scheduling-algorithms)
- [The execution engine](#the-execution-engine)
- [gRPC API](#grpc-api)
- [Python bindings](#python-bindings)
- [Desktop client](#desktop-client)
- [Testing](#testing)
- [Benchmarks](#benchmarks)
- [Configuration](#configuration)
- [Limitations](#limitations)
- [References](#references)
- [Licence](#licence)

---

## What this is, and what it is not

It is:

- a single-symbol limit order book with price-time priority, integer tick
  prices, GTC/IOC/FOK and market orders, and a full internal-consistency check;
- four scheduling algorithms — TWAP, VWAP, POV and Almgren–Chriss
  implementation shortfall — as pure functions over integers and `f64`, with the
  closed forms derived in the code comments;
- a deterministic execution engine: given the same inputs and the same
  timestamps, it produces the same fills, because it never reads the clock
  itself;
- exact position and P&L accounting in `i128`, with the identity
  `realized_pnl − cost_basis = net cash` checked by a property test;
- pre-trade risk checks and a loss kill switch;
- a gRPC service, a desktop client, and Python bindings for the algorithms.

It is not:

- connected to any exchange. Orders execute against a **simulated venue** in
  this process: a market maker quotes a ladder around a fixed reference price
  per symbol, fills consume that depth, and the next engine tick restores it.
  The mid does not move in response to anything, so P&L out of the simulator
  measures the scheduler against a static book, not against a market.
- a backtester. There is no historical replay, no commission model and no
  slippage model. An earlier version of this repository contained all three;
  they were removed because they were wrong (commissions were configured and
  never charged, signals executed at the same bar's close that produced them,
  and short positions added to equity instead of subtracting).
- low latency in any end-to-end sense. The figures under
  [Benchmarks](#benchmarks) are in-process microbenchmarks of book operations on
  one machine. Nothing here has been measured across a network, and no part of
  the system is lock-free.
- authenticated. The gRPC server has no authentication or TLS; keep it on a
  loopback address, which is what the shipped configuration does.

---

## Build and run

```bash
git clone https://github.com/shaunak-batra/RustAlgoExecutionSystem.git
cd RustAlgoExecutionSystem

cargo build --release
cargo test --workspace          # 170 tests
```

`protoc` is vendored through `protoc-bin-vendored`, so a clean clone builds with
no system protobuf installation and no generated files in version control.

Run the engine, then the client:

```bash
cargo run --release --bin engine                      # serves config/default.toml
cargo run --release --bin engine -- --config my.toml  # or your own settings
cargo run --release --bin trader-gui                  # connects to 127.0.0.1:50051
```

The engine logs through `tracing`; set `RUST_LOG=debug` for more detail. It
shuts down on Ctrl-C, draining in-flight commands first.

Run the tests in both profiles. The debug profile keeps integer-overflow checks
on, and release exercises optimisation-dependent behaviour:

```bash
cargo test --workspace
cargo test --workspace --release
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

---

## Architecture

```text
            ┌──────────────────────┐        ┌───────────────────────┐
            │   trader-gui         │        │  python/              │
            │   desktop client     │        │  algo_exec_py         │
            └──────────┬───────────┘        └───────────┬───────────┘
                       │ gRPC                           │ PyO3
            ┌──────────▼───────────┐                    │
            │   api                │                    │
            │   tonic service      │                    │
            └──────────┬───────────┘                    │
                       │ mpsc commands, oneshot replies │
                       │ broadcast fills                │
            ┌──────────▼───────────────────────────┐    │
            │   engine                             │    │
            │   EngineCore: submit / cancel / tick │    │
            │   accounting · risk · venue · state  │    │
            └──────────┬───────────────┬───────────┘    │
                       │               │                │
            ┌──────────▼─────┐  ┌──────▼────────────────▼───┐
            │   orderbook    │  │   algo-core               │
            │   matching     │  │   TWAP VWAP POV IS        │
            └────────────────┘  └───────────────────────────┘
```

Seven crates, each with a single responsibility:

| Crate | Responsibility |
| --- | --- |
| `orderbook` | Limit order book, price/quantity/timestamp types. Depends only on `serde` and `thiserror`. |
| `algo-core` | The four schedulers as pure functions. No I/O, no clock. |
| `engine` | `EngineCore` state machine, exact accounting, pre-trade risk, the simulated venue, and the async actor that owns them. |
| `api` | Protobuf definitions and the tonic service; translates requests into engine commands. |
| `config` | TOML settings with unknown-key rejection and range validation. |
| `python-bindings` | PyO3 wrapper around `algo-core`, built as `algo_exec_py._native`. |
| `trader-gui` | egui client. Displays engine state only. |

---

## The order book

`orderbook::OrderBook` is a single-symbol book. Bids and asks are
`BTreeMap<Price, Level>`; each level holds a `VecDeque<Order>` in arrival order;
a `HashMap<OrderId, (Side, Price)>` index means a cancel never scans the book.

- Prices are integer ticks (`TICK_SCALE = 100_000`, five decimal places), so
  level lookup and comparison are exact and floating point appears only at the
  boundaries. `Price::try_from_f64` rejects NaN, infinities and anything whose
  rounded tick count reaches either end of the `i64` range, symmetrically.
- A trade executes at the resting order's price. The best level fills first;
  within a level, earlier orders fill first; a partially filled resting order
  keeps its queue position.
- Time in force decides the outcome, for limit and market orders alike: a GTC
  remainder rests, an IOC remainder is cancelled, and a FOK order either trades
  in full immediately or does nothing at all.
- `check_invariants()` verifies, in expected `O(n)`: no empty levels, no id
  resting twice, cached level totals equal to the sum of their orders, an id
  index matching the resting orders exactly, every resting order a GTC limit
  order on the correct side and level, and an uncrossed book.

Complexity, with `L` levels on a side and `k` orders at one level. Index work is
hash-map work, so those bounds are expected and amortised:

| Operation | Bound |
| --- | --- |
| `submit_order` | `O(log L)` per level touched, plus expected amortised `O(1)` per order filled or rested |
| `cancel_order` | expected `O(1)` index lookup, `O(log L)` level lookup, `O(k)` within the level |
| `best_bid` / `best_ask` | `O(log L)` |

How it is checked: 38 unit tests, one doctest, and a property test that replays
up to 79 random operations against a deliberately naive reference
implementation, comparing every execution report, every cancel, full depth on
both sides, the order count and the invariants after each step
(`crates/orderbook/tests/reference_model.rs`, 512 cases).

---

## The scheduling algorithms

All four return `Result`, validate their inputs, and never panic. Quantities are
integers that sum exactly to the order size — for POV, to what the observed
volume allowed.

### TWAP

Equal quantities per time slice, to within one unit. Slice `k` starts at
`start + ⌊k · duration / slices⌋`. The quantity complete after slice `k` is
`total · (k+1) / slices` rounded to the nearest unit with ties up, computed in
`u128` as `q·k + ⌊r·k/n⌋ + [2(r·k mod n) ≥ n]`, so nothing overflows even at
`u64::MAX` over a million slices, and any remainder is spread across the window
instead of front-loaded. Slices that round to zero are omitted.

`compute_twap_randomized` draws each release time uniformly inside its own slice
using SplitMix64, so the schedule is reproducible from the seed on any platform
while release times are not predictable from a fixed clock. One draw is made per
slice, including slices whose quantity rounds to zero, so slice `k`'s time
depends on the seed, the window, the slice count and `k` — never on the total
quantity. A pinned golden schedule and an independent Python implementation of
the generator both check this.

### VWAP

Quantities in proportion to a volume profile, one non-negative weight per slice.
The cumulative target is rounded rather than each slice independently, which
keeps the rounding error from accumulating: prefix `k` lands within half a unit
of `total × cumulative[k]` as evaluated in `f64`, plus the error of that
evaluation. Because the profile's cumulative shares are themselves `f64` sums,
that second term grows with the slice count at very large totals — the
documentation states the bound it actually has rather than an unconditional half
unit. The curve stops at the last slice with volume, so a zero-weight slice
never receives a child order, including above `2^53` where a rounded `f64` target
cannot reach the total exactly.

`volume_profile_from_bars` pools historical bars by time of day across days and
returns each slice's share, so a busier day weighs more. Its fractions sum to 1
up to floating-point rounding.

### POV

Participation in integer basis points of observed volume. After each observation
inside the window the cumulative target is
`min(total, ⌊cumulative volume × bps / 10 000⌋)`, computed in `u128`, and any
increase is released at that observation's timestamp. Exact integer arithmetic
means the cumulative scheduled quantity can never exceed the participation rate
times cumulative volume. Quantity the volume did not permit is reported as
`shortfall_qty` rather than silently dropped.

The timing is a convention about the input, which the documentation says
plainly: a child is released at the timestamp of the observation that allowed
it, so `timestamp_ns` must be when the volume became known — a bar's close, not
its open. A property test checks prefix invariance: once every observation
carrying a timestamp has been seen, that timestamp's child is settled and later
data cannot change it.

### Implementation shortfall (Almgren–Chriss)

Trading `X` units over `N` slices of length `τ`, with holdings
`x_0 = X, …, x_N = 0` and `n_k = x_{k−1} − x_k`:

```text
E = ½γX² + (η̃/τ)·Σ n_k²        η̃ = η − ½γτ
V = σ²τ·Σ_{k≥1} x_k²
```

Minimising `E + λV` gives `x_j = X·sinh(κ(T − t_j)) / sinh(κT)` with `κ` solving
`(2/τ²)(cosh(κτ) − 1) = λσ²/η̃`. The schedule is the same for buying and
selling; the fixed spread cost `ε` is omitted because it adds `ε·X` to every
one-directional schedule and so cannot change the optimum.

Numerically: `κτ = acosh(1 + y)` is evaluated as `ln_1p(y + √(y(y+2)))`, which
keeps the `√(2y)` behaviour for tiny `y` where `1 + y` rounds to 1, and factors
`y` out of the square root for huge `y` where `y(y+2)` would overflow and reject
a `κ` that is perfectly finite. The `sinh` ratio is evaluated through `exp_m1`
so that `κT` in the hundreds does not produce `inf/inf`.

With `λ = 0` the trajectory is exactly linear and the schedule is TWAP, down to
the same integer child quantities. That equivalence is delegated to TWAP's
rounding rather than re-derived from the float curve, because rounding
`1 − x/total` lands on the wrong side of an exact half for sizes such as 9 units
over 6 slices. A sweep over 39 slice counts × 203 sizes checks it.

Correctness is pinned by more than a regression test: the holdings match the
textbook `sinh` formula for `λ` from `1e-11` to `1e-1`; `κ` satisfies the
discrete urgency equation to within `1e-9` relative; the first-order optimality
condition `(η̃/τ²)(x_{k−1} − 2x_k + x_{k+1}) = λσ²x_k` holds at every interior
point; the closed-form cost matches an independent summation; and a trajectory
rebuilt with `κ` off by 5% costs strictly more.

---

## The execution engine

`EngineCore` is a single-owner state machine. Its entire API is `submit`,
`cancel`, `tick`, `halt`, `positions` and `order_status`, and every decision is
a function of the timestamp passed in — it never reads the clock. That is what
makes a run reproducible: the same commands with the same timestamps produce the
same fills, which a determinism test asserts directly.

Around it, `ExecutionEngine` is an async actor: commands arrive on an `mpsc`
channel, replies go back on `oneshot` channels, and fills fan out on a
`broadcast` channel. There are no locks and no shared mutable state, because one
task owns everything.

On each tick the engine releases every child order that has come due, as an IOC
order, and carries any unfilled quantity forward to the next slice rather than
abandoning it. Child orders are stamped with the time they are actually
submitted, not the time they were scheduled for. A slice whose quantity rounds
to zero is skipped rather than sent to the book.

**Accounting** is exact. Positions and P&L are `i128` in units of price ticks ×
quantity, including the flip case where a fill crosses through zero and part of
it closes the old position while the rest opens a new one. A property test
asserts the identity `realized_pnl − cost_basis = net cash` over random fill
sequences, and an integration test checks a round trip where the numbers are
known exactly.

**Risk** is checked before an order is accepted: per-order quantity; per-order
notional priced at the worse of the limit price and the mid; a price collar in
basis points of the mid; the absolute position per symbol assuming every working
order fills; and gross exposure across symbols. Trading halts once total P&L
(realised plus unrealised at the mid) reaches the configured loss limit, and the
halt reason is reported over the API.

---

## gRPC API

`proto/execution.proto` defines five calls. The schema uses typed enums, a
`oneof` for the algorithm parameters, and `optional` fields rather than sentinel
values such as a zero price meaning "no limit".

| Call | Returns |
| --- | --- |
| `SubmitParentOrder` | `accepted`, the new id, or a `reject_reason` |
| `CancelParentOrder` | whether the remaining quantity was cancelled, or why not |
| `GetOrderStatus` | state, filled quantity, arrival mid, average fill price, shortfall in bps, slices still to release, and every child order |
| `GetPositions` | a report per symbol, plus whether risk has halted trading and why |
| `StreamFills` | a server stream of fill events |

Error semantics are deliberate, because conflating these two is a common way for
a client to mis-handle rejections:

- A **business rejection** — failing a risk limit, an unknown symbol, an invalid
  window — returns `accepted = false` with a reason, as a successful RPC.
- A **malformed request** returns `INVALID_ARGUMENT`; an unknown order id
  returns `NOT_FOUND`; the engine being gone returns `UNAVAILABLE`.
- A fill subscriber that falls behind has its stream ended with `DATA_LOSS` and a
  message telling it to resubscribe and reconcile through `GetOrderStatus`,
  rather than silently missing fills.

POV is absent from this API on purpose. `algo_core::pov` is implemented and
tested, but the simulated venue publishes no per-order volume feed for a
participation rate to track, so POV is reachable from the library and the Python
bindings instead.

---

## Python bindings

A PyO3 extension built with maturin in a mixed layout: the pure-Python package
`algo_exec_py` ships with the compiled module installed inside it as
`algo_exec_py._native`.

```bash
pip install -r python/requirements.txt
pip install ./python
pytest python/tests -q          # 67 cases
```

```python
import algo_exec_py as ae

# (target_time_ns, qty) per child order
ae.twap_schedule(start_ns=0, end_ns=10_000_000_000, total_qty=1_000, num_slices=10)
ae.twap_schedule_randomized(0, 10_000_000_000, 1_000, 10, seed=42)

profile = ae.volume_profile_from_bars(bars, start_offset_ns, duration_ns, num_slices)
ae.vwap_schedule(0, 10_000_000_000, 1_000, profile)

# dicts: children, scheduled_qty, shortfall_qty
ae.pov_schedule(0, 1_000, 10_000, 1_000, [(1, 500), (2, 500)])

# dict: children, holdings, kappa, expected_cost, cost_variance
ae.almgren_chriss_schedule(
    start_ns=0, end_ns=3_600_000_000_000, total_qty=100_000, num_slices=12,
    risk_aversion=1e-3, volatility=0.02, temporary_impact=0.5, permanent_impact=1e-6,
)
```

Which exception a bad argument raises depends on how it is wrong, because
arguments are converted to Rust types before any scheduler runs: the wrong type
raises `TypeError`, an integer outside its parameter's range raises
`OverflowError`, and a value the scheduler rejects raises `ValueError` carrying
the Rust error message.

`requires-python` is `>=3.9,<3.14`: PyO3 0.22 supports CPython 3.13 at the
newest, and without the cap `pip` on 3.14 starts a build that then fails in the
compiler. CI runs both ends of that range.

The test suite compares the bindings against independent pure-Python
implementations of the same definitions — integer TWAP rounding, SplitMix64 and
the multiply-shift draw, the Almgren–Chriss closed form, the POV cap — so a
mistake in the Rust shows up as a mismatch rather than as two copies of the same
error agreeing.

---

## Desktop client

An egui application that is a thin client of the engine. Every value it shows
comes from a gRPC reply: parent order state and child orders, positions with
their mark prices, streamed fills, and the risk halt. It holds no market data
source of its own and simulates nothing.

It offers TWAP, VWAP and implementation shortfall — the three the API accepts —
with the parameters each one actually takes, and it sends the algorithm that is
selected. Limit prices are converted through `Price::try_from_f64`, so the field
means what it says. Form parsing and every unit conversion are pure functions
with unit tests (15 of them), which is why a bug like sending a price in whole
units instead of ticks now fails a test.

---

## Testing

170 Rust tests, which must pass in debug and release, with
`clippy --workspace --all-targets -- -D warnings` clean:

| Crate | Unit | Doc | Integration |
| --- | --- | --- | --- |
| `orderbook` | 38 | 1 | 1 (reference model, 512 random op sequences) |
| `algo-core` | 58 | 5 | — |
| `engine` | 21 | 1 | 18 (17 core scenarios, 1 end-to-end over gRPC) |
| `api` | 5 | 1 | — |
| `config` | 4 | — | — |
| `python-bindings` | 2 | — | — |
| `trader-gui` | 15 | — | — |
| **Total** | **143** | **8** | **19** |

Plus 34 Python test functions, 67 cases after parametrisation.

The kinds of checking that matter more than the count:

- **A differential test against a naive model.** The order book is compared
  operation by operation with an implementation written for obviousness rather
  than speed.
- **Property tests** for the invariants that should hold for every input: the
  accounting identity, schedule totals and monotonicity, the POV participation
  cap, prefix invariance, and first-order optimality of the Almgren–Chriss
  trajectory.
- **Known-answer tests** where a value can be derived independently: the first
  output of `splitmix64.c`, a pinned randomized TWAP schedule, closed-form cost
  and variance, and an exact round-trip P&L.
- **Mutation testing.** Twenty deliberate single-line mutations — each one a
  plausible mistake that the suite previously failed to notice, plus a revert of
  each bug fixed in this pass — were applied and the suite was confirmed to fail
  for every one. The mutants covered the randomized-TWAP draw, the multiply-shift
  reduction, uneven slice widths, the monotonicity clamp, tie rounding, the VWAP
  truncation and window boundaries, the small-`κ` and large-`κ` paths, and the
  `η̃ > 0` precondition.
- **An end-to-end test over a real socket**, covering submit, stream, status and
  positions, and checking that a business rejection and a malformed request are
  reported differently.

---

## Benchmarks

Criterion, `bench` profile (release with LTO), on one developer machine —
a single-threaded, in-process measurement of book operations, not an end-to-end
latency figure. Reproduce with `cargo bench -p orderbook`.

| Benchmark | Levels per side | Time |
| --- | --- | --- |
| Insert a passive limit order, then cancel it | 10 | 69.5 ns |
| Insert a passive limit order, then cancel it | 1 000 | 88.6 ns |
| IOC takes the best ask, then a passive order restores the level | 10 | 131.9 ns |
| IOC takes the best ask, then a passive order restores the level | 1 000 | 152.9 ns |
| Read best bid and best ask | 1 000 | 4.6 ns |
| 100 000 mixed operations | 100 | 11.1 ms, 9.0 M ops/s |

The per-operation benchmarks restore the book to the same shape after every
iteration, so the measured cost does not drift as the benchmark runs.

The mixed workload is generated against a shadow copy of the book, so every
operation is one the book can really act on: cancels name an order that is still
resting at that point in the stream, and each IOC is priced through the live
touch so it actually trades. The add/cancel choice holds the resting count
inside a band, and the benchmark asserts the book ends near the size it started
at — otherwise the per-operation figure would be an average over a book that
grew as the stream replayed. With seed 7 the realised stream is 48 250 passive
limit orders, 36 830 cancels and 14 920 IOC orders producing 20 956 fills, with
the resting count going from 800 to 899.

---

## Configuration

`config/default.toml`, loaded by the engine. Unknown keys are rejected, so a
misspelled setting fails loudly instead of falling back to a default, and every
value is range-checked with the offending field named in the error.

```toml
[engine]
tick_interval_ms = 50          # how often due children are released and quotes refreshed
command_buffer = 1024
fill_buffer = 4096             # how far a fill subscriber may fall behind
retained_finished_orders = 10000

[api]
listen_addr = "127.0.0.1:50051"   # no authentication; keep it on loopback

[risk]
max_order_qty = 100000
max_order_notional = 10000000.0
max_position_qty = 500000
max_gross_notional = 50000000.0
price_collar_bps = 500
max_loss = 250000.0            # total P&L at which trading halts

[[venue.symbols]]              # the simulated market maker's ladder
symbol = "BTC-USD"
reference_price = 65000.0
levels = 20
qty_per_level = 5
level_spacing_bps = 1
```

Three symbols are configured: `BTC-USD`, `ETH-USD` and `SIM-EQ`. Money amounts
are given in price units and converted to integer ticks by the engine.

---

## Limitations

Stated because they are the first things worth asking about:

- The venue is simulated and its reference price never moves. Fills consume
  depth; the next tick restores it. Execution cost measured here reflects the
  scheduler walking a static book.
- No exchange connectivity, no historical replay, no commissions or fees, no
  borrow or financing costs, no partial-fill latency model, no queue-position
  modelling beyond arrival order.
- Self-trade prevention is not modelled: the book has no notion of order owners.
- The gRPC API has no authentication, authorisation or TLS.
- Nothing is persisted. Engine state lives in memory for the life of the
  process.
- POV is not exposed through the engine API, for the reason given above.
- The loss kill switch halts new orders; it does not liquidate existing
  positions.
- Benchmarks are single-machine and in-process. No throughput claim is made for
  the engine or the API as a whole, because none has been measured.

---

## References

- Almgren, R. and Chriss, N. (2000). "Optimal execution of portfolio
  transactions." *Journal of Risk* 3(2), 5–39. The implementation shortfall
  scheduler follows the discrete linear-impact model from this paper.
- Steele, G., Lea, D. and Flood, C. (2014). "Fast splittable pseudorandom number
  generators." The `splitmix64` variant used for randomized TWAP release times.

---

## Licence

MIT. See [LICENSE](LICENSE).
