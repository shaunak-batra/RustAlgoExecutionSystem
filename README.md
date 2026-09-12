# Algorithmic execution engine

This is an order execution system written in Rust. You give it a parent order —
buy 100 000 units of something over the next hour — and it breaks that into
child orders, sends them to a venue, tracks what came back, and tells you what
the execution cost you against the price at the moment you decided to trade.

It is not a strategy engine. There is no alpha here, no signal, no prediction.
The question it answers is the one that comes after you have already decided to
trade: *how* do you trade it.

I have tried to make every statement in this file checkable against the code.
Where the system is simulated or a feature is missing, I say so.

---

## Contents

- [Why I built this](#why-i-built-this)
- [The problem, concretely](#the-problem-concretely)
- [How it fits together](#how-it-fits-together)
- [The order book](#the-order-book)
- [The four algorithms](#the-four-algorithms)
- [The engine](#the-engine)
- [The gRPC API](#the-grpc-api)
- [The desktop client](#the-desktop-client)
- [Python bindings](#python-bindings)
- [How I know it works](#how-i-know-it-works)
- [Results](#results)
- [What I got wrong](#what-i-got-wrong)
- [What it doesn't do](#what-it-doesnt-do)
- [Running it](#running-it)
- [References](#references)
- [Licence](#licence)

---

## Why I built this

I wanted to understand execution properly, and the only way I know to check that
I understand something is to implement it and then try to break it.

Execution is a good subject for that because it is mathematically concrete. A
TWAP schedule either sums to the right quantity or it doesn't. The Almgren–Chriss
trajectory either satisfies the first-order optimality condition or it doesn't.
A position either reconciles with cash or it doesn't. There is very little room
to wave your hands, which is exactly what I wanted.

The second reason is that I had written a version of this before that looked
impressive and wasn't. The README claimed 100 000 orders per second, microsecond
latency, smart order routing, iceberg orders, a market data service with
WebSocket failover. Most of that did not exist. The parts that did exist were
often wrong in ways no test would catch, because the tests asserted things like
"the result is between 0 and 100". Going back through it and making the code and
the claims agree taught me more than writing it did.

---

## The problem, concretely

Say you want to buy 100 000 units and the book looks like this:

```text
        asks                        you send one market order for 100 000
  101.50 ┤████ 40 000                        │
  101.00 ┤███ 30 000                         │  fills 20 000 @ 100.50
  100.50 ┤██ 20 000   <- best ask  <─────────┘  fills 30 000 @ 101.00
  ───────┼─────────────────────────             fills 40 000 @ 101.50
  100.00 ┤██ 25 000   <- best bid              fills 10 000 @ next level up
   99.50 ┤███ 35 000
```

You have just paid an average well above 100.50, and you have moved the price
against yourself while doing it. Every unit after the first 20 000 cost more
because you were the one buying.

The alternative is to spread the order out, so that you are taking a small slice
of the available liquidity at a time and the book has a chance to refill between
your orders:

```text
  one market order           sliced over an hour
  ┌────────────┐             ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐
  │  100 000   │             │16│ │17│ │16│ │17│ │17│ │17│
  └────────────┘             └──┘ └──┘ └──┘ └──┘ └──┘ └──┘
   ↑                          10m  20m  30m  40m  50m  60m
   pays through four levels    each one sits inside the spread
```

That is the whole idea. The cost of slicing is that you are exposed to the price
moving while you wait, which is the tension every one of these algorithms is
trying to balance. TWAP ignores it. Implementation shortfall prices it
explicitly.

---

## How it fits together

```text
   ┌──────────────────────┐      ┌────────────────────────┐
   │  trader-gui          │      │  python/algo_exec_py   │
   │  egui desktop client │      │  PyO3 bindings         │
   └──────────┬───────────┘      └────────────┬───────────┘
              │ gRPC                          │ direct calls
   ┌──────────▼───────────┐                   │
   │  api                 │                   │
   │  tonic service       │                   │
   └──────────┬───────────┘                   │
              │ mpsc commands                 │
              │ oneshot replies               │
              │ broadcast fills               │
   ┌──────────▼─────────────────────────────┐ │
   │  engine                                │ │
   │                                        │ │
   │   EngineCore   ── deterministic        │ │
   │   accounting   ── exact i128 P&L       │ │
   │   risk         ── pre-trade + halt     │ │
   │   venue        ── simulated market     │ │
   │   state        ── parent/child orders  │ │
   └──────────┬───────────────┬─────────────┘ │
              │               │               │
   ┌──────────▼─────┐  ┌──────▼───────────────▼──┐
   │  orderbook     │  │  algo-core              │
   │  matching      │  │  TWAP VWAP POV IS       │
   │  price types   │  │  pure functions         │
   └────────────────┘  └─────────────────────────┘
```

Seven crates:

| Crate | What it does |
| --- | --- |
| `orderbook` | The limit order book and the price, quantity and timestamp types. Depends on `serde` and `thiserror`, nothing else. |
| `algo-core` | The four schedulers. Pure functions — no clock, no I/O, no allocation surprises. |
| `engine` | The state machine, exact accounting, risk checks, the simulated venue, and the async task that owns them. |
| `api` | The protobuf schema and the tonic service. Translation only, no logic. |
| `config` | TOML settings, with unknown keys rejected and every value range-checked. |
| `python-bindings` | PyO3 wrapper, built as `algo_exec_py._native`. |
| `trader-gui` | The desktop client. Shows engine state and nothing else. |

I split it this way for one reason: I wanted to be able to test each layer
without the layer above it. The algorithms can be tested with no clock, because
they take timestamps as arguments. The engine can be tested with no network,
because the transport is a separate crate. The API can be tested with no GUI. If
a test needs three layers running to say anything, it is hard to know what broke
when it fails.

---

## The order book

A single symbol. Bids and asks are each a `BTreeMap` from price to a level, and a
level is a queue of orders in arrival order:

```text
  OrderBook
  ├── bids: BTreeMap<Price, Level>          (highest price = best)
  │     100.00 ─> Level { total_qty: 45, orders: [#3 qty 25, #7 qty 20] }
  │      99.50 ─> Level { total_qty: 35, orders: [#1 qty 35] }
  │
  ├── asks: BTreeMap<Price, Level>          (lowest price = best)
  │     100.50 ─> Level { total_qty: 20, orders: [#4 qty 20] }
  │     101.00 ─> Level { total_qty: 30, orders: [#2 qty 10, #9 qty 20] }
  │
  └── index: HashMap<OrderId, (Side, Price)>
        #1 -> (Buy, 99.50)   #2 -> (Sell, 101.00)   ...
```

The index is there so a cancel does not have to search. Given an id you know
which side and which price level to go to, and then you only scan that one
level.

**Matching rules.** A trade executes at the *resting* order's price, not the
incoming one — the person who was there first gets their price. The best price
level fills first, and within a level the earliest order fills first. An order
that is partially filled keeps its place in the queue rather than going to the
back.

**Time in force** decides what happens to whatever is left over, and it works
the same way for limit and market orders:

```text
  GTC  ── the remainder rests in the book
  IOC  ── trade what is there, cancel the rest
  FOK  ── trade the whole quantity right now, or do nothing at all
```

**Why prices are integers.** One tick is 1/100 000 of a currency unit. Prices
are `i64` tick counts, so comparing two prices and looking up a level are exact
operations. Floating point shows up only at the edges, when a human types a
price in or reads one out. I did this after watching `0.29 * 100_000.0` come out
as `28999.999999999996` — if you truncate that you land on the wrong tick, and
from then on your order is sitting at a price nobody asked for.

**`check_invariants()`** walks the whole book and verifies: no empty levels, no
id resting twice, every cached level total equal to the sum of its orders, the
index matching the resting orders exactly, every resting order a GTC limit order
on the correct side and level, and the book not crossed. Tests call it after
every operation, which is how I catch a structural bug at the moment it happens
instead of three operations later.

Complexity, with `L` levels on a side and `k` orders at one level. Anything that
touches the index is a hash map, so those bounds are expected and amortised, not
worst case:

| Operation | Cost |
| --- | --- |
| `submit_order` | `O(log L)` per level touched, plus expected amortised `O(1)` per order filled or rested |
| `cancel_order` | expected `O(1)` index lookup, `O(log L)` level lookup, `O(k)` inside the level |
| `best_bid` / `best_ask` | `O(log L)` |

---

## The four algorithms

All four are pure functions that return a `Result`. They validate their input
and never panic. Quantities are integers that sum exactly to the order size.

### TWAP — time-weighted average price

Split the quantity evenly across equal time slices.

```text
  total 100, 5 slices, one hour
  ┌────┬────┬────┬────┬────┐
  │ 20 │ 20 │ 20 │ 20 │ 20 │
  └────┴────┴────┴────┴────┘
  0m   12m  24m  36m  48m

  total 103, 5 slices  — the 3 extra units get spread out, not front-loaded
  ┌────┬────┬────┬────┬────┐
  │ 21 │ 20 │ 21 │ 20 │ 21 │
  └────┴────┴────┴────┴────┘
```

The rounding is the interesting part. The naive way is `base = total / n` and
then hand the remainder to the first few slices, which front-loads the order —
the opposite of what TWAP is for. Instead I round the *cumulative* target:
after slice `k` you should have done `total · (k+1) / n` units, rounded to the
nearest unit. Differencing those cumulative targets gives the per-slice
quantities, the error never accumulates, and the remainder lands spread across
the window.

It is computed in `u128` as `q·k + ⌊r·k/n⌋ + [2(r·k mod n) ≥ n]`, which means
nothing overflows even at `u64::MAX` units over a million slices. There is a
test for exactly that.

**Randomized TWAP** draws each release time uniformly inside its own slice,
using SplitMix64 seeded by a number you pass in. Same seed, same schedule, on
any platform — so it is reproducible — but an observer watching a fixed clock
cannot predict when the next child order is coming.

```text
  slice boundaries   │        │        │        │        │
  plain TWAP         ▲        ▲        ▲        ▲        ▲
  randomized           ▲          ▲  ▲            ▲   ▲
```

One draw happens per slice, including slices whose quantity rounds to zero. That
sounds like a detail and isn't: it means slice `k`'s release time depends on the
seed, the window and the slice count, but never on the total quantity. If the
draw were skipped for empty slices, changing the order size would shift the
timing of every later slice. There is a test comparing a 5-unit order and a
100-unit order over the same window and asserting the shared slices keep their
times.

### VWAP — volume-weighted average price

Trade in proportion to when volume normally happens, so you are a roughly
constant share of the market rather than a constant share of the clock.

```text
  typical intraday volume        the schedule that follows it
  █                    █         ┌──┐              ┌──┐
  █                    █         │28│              │28│
  █ █              █   █         │  │ ┌─┐      ┌─┐ │  │
  █ █ ▄  ▄  ▄  ▄   █   █         │  │ │14│ │7│ │14│ │  │
  ─────────────────────────      └──┘ └──┘ └─┘ └──┘ └──┘
  open          midday   close
```

`volume_profile_from_bars` builds the profile by pooling historical bars by time
of day across many days, so a busier day weighs more. Then the schedule follows
the cumulative share of that profile.

Two things I had to be careful about. The cumulative targets are rounded, same
as TWAP, so the error does not accumulate. And the curve *stops* at the last
slice that actually has volume, rather than being forced to 1.0 from there
onwards. That sounds equivalent and is not: above 2^53 units, rounding a float
target cannot land exactly on the total, so the forced version handed the
leftover to a trailing zero-volume slice — an order in a slice the profile said
had no volume in it. Truncating the curve means a zero-weight slice never gets a
child order, at any size.

### POV — percentage of volume

Participate at a fixed share of whatever volume actually prints.

```text
  volume prints:   500    500    200    800
  at 10%:           50     50     20     80    units released
                     ▲      ▲      ▲      ▲
                     │      │      │      │
                  each child released at the timestamp of
                  the print that allowed it
```

The rate is in integer basis points and the cap is computed in `u128` as
`min(total, ⌊cumulative volume × bps / 10 000⌋)`. Because it is exact integer
arithmetic, the cumulative quantity you have scheduled can never exceed your
participation rate times the cumulative volume — not approximately, ever. If the
volume never showed up, the quantity you could not trade is reported as
`shortfall_qty` instead of being quietly dropped.

One honest note about timing, which I put in the code as well. A child is
released at the timestamp of the observation that allowed it. So the timestamps
you feed in have to be the moment the volume became *known* — a bar's close, not
its open. The function cannot check that for you. If you pass bar opens, you have
built a look-ahead, and no amount of care inside the function will save you.

### Implementation shortfall — Almgren–Chriss

This is the one with real mathematics in it. The other three are rules; this one
is an optimisation.

Trading `X` units over `N` slices of length `τ`, holdings `x_0 = X … x_N = 0`,
trading `n_k = x_{k−1} − x_k` in slice `k`:

```text
  expected cost    E = ½γX² + (η̃/τ)·Σ n_k²        where η̃ = η − ½γτ
  variance         V = σ²τ·Σ x_k²

  γ = permanent impact    η = temporary impact    σ = volatility
```

The first term is what you move the price by permanently and cannot avoid. The
second is what you pay for trading in a hurry. The variance is the risk you take
by still holding a position while the price wanders.

Minimising `E + λV`, where `λ` is how much you dislike that risk, gives

```text
  x_j = X · sinh(κ(T − t_j)) / sinh(κT)

  with κ solving   (2/τ²)(cosh(κτ) − 1) = λσ²/η̃
```

What that looks like in practice:

```text
  λ = 0 (don't care about risk)     λ large (get it done)
  ┌──┬──┬──┬──┬──┬──┐              ┌──────┐
  │17│17│17│17│17│17│              │  52  │┌──┐
  └──┴──┴──┴──┴──┴──┘              │      ││24│┌─┐
   evenly spread = TWAP            │      ││  ││12│┌─┐ ┌─┐ ┌─┐
                                   └──────┘└──┘└──┘└─┘ └─┘ └─┘
                                    front-loaded, pays more impact
                                    to spend less time exposed
```

Three numerical things mattered more than I expected:

`κτ = acosh(1 + y)` is written as `ln_1p(y + √(y(y+2)))`. For tiny `y`, `1 + y`
rounds to exactly 1 and `acosh` would hand back 0, killing the urgency entirely.
The `ln_1p` form keeps the `√(2y)` behaviour. For huge `y`, `y(y+2)` overflows
to infinity, and the code used to reject a `κ` that was actually about 1.55 as
"too large" — so the large branch factors `y` out of the square root.

The `sinh` ratio is evaluated through `exp_m1` rather than calling `sinh` twice.
At high urgency `κT` reaches the hundreds, where `sinh(κT)` is infinity and the
ratio becomes `inf/inf` — a NaN schedule.

With `λ = 0` the trajectory is exactly linear, so the schedule should be TWAP.
It wasn't. Rounding `1 − x/total` in floating point lands on the wrong side of an
exact half for sizes like 9 units over 6 slices, giving `1,2,2,1,2,1` where TWAP
gives `2,1,2,1,2,1`. When `κ` is zero the code now uses TWAP's integer rounding
directly, and a sweep over 39 slice counts × 203 sizes checks the two agree.

---

## The engine

`EngineCore` is a state machine with six entry points: `submit`, `cancel`,
`tick`, `halt`, `positions`, `order_status`. It never reads the clock. Every
decision is a function of a timestamp handed to it.

That one constraint is the most useful thing in the project. Because the core
cannot read the clock, the same commands with the same timestamps produce the
same fills, every time, and a test can assert that directly. A system that calls
`Instant::now()` in the middle of its logic cannot be tested that way, and I
have written enough of those to want the alternative.

Around the core sits an async actor:

```text
  gRPC handler ──mpsc command──> ┌─────────────┐
                                 │  one task   │
  gRPC handler <──oneshot reply──│  owns       │
                                 │  EngineCore │
  fill stream  <──broadcast─────│             │
                                 └──────┬──────┘
                        tick every 50ms │
```

One task owns all the state, so there are no locks and nothing is shared. Work
arrives as messages and replies go back on a channel created for that one
request.

What a tick does:

```text
  for each working parent order:
      has the window ended?  ──yes──> finish it
                             ──no───> is a slice due?
                                        ──yes──> release child as IOC
                                                 carry any unfilled qty forward
      refresh the market maker's quotes
      mark positions, check the loss limit
```

Child orders go out as IOC, and anything that doesn't fill is carried into the
next slice rather than abandoned. Children are stamped with the time they were
actually sent, not the time they were scheduled for — a fill should tell you
when it happened.

**Accounting is exact.** Positions and P&L are `i128` in units of price ticks ×
quantity, including the awkward case where a fill crosses through zero:

```text
  you are short 30, you buy 50
  ─────────────────────────────
  the first 30 close the short  ──> realised P&L
  the next 20 open a long       ──> new cost basis
```

A property test throws random fill sequences at it and checks
`realized_pnl − cost_basis = net cash` every time. If that identity holds, the
books balance; if it ever doesn't, money has appeared or vanished.

**Risk** runs before an order is accepted: per-order quantity, per-order notional
priced at the worse of your limit and the mid, a price collar in basis points,
the position you would have if every working order filled, and gross exposure
across symbols. Separately, if total P&L reaches the configured loss limit,
trading halts and the API reports why.

---

## The gRPC API

Five calls, defined in `proto/execution.proto` with typed enums, a `oneof` for
the algorithm parameters, and `optional` fields instead of magic values like
"price zero means market".

| Call | What comes back |
| --- | --- |
| `SubmitParentOrder` | accepted plus an id, or a reject reason |
| `CancelParentOrder` | whether the remaining quantity was cancelled, or why not |
| `GetOrderStatus` | state, filled quantity, arrival mid, average fill price, shortfall in bps, slices left, every child order |
| `GetPositions` | a report per symbol, plus whether risk has halted trading and why |
| `StreamFills` | a server stream of fills |

The error semantics are deliberate, because mixing these up is how a client ends
up retrying something it should have shown the user:

```text
  risk limit hit, unknown symbol, bad window
      ──> accepted = false + reason   (the RPC itself succeeded)

  malformed request    ──> INVALID_ARGUMENT
  unknown order id     ──> NOT_FOUND
  engine gone          ──> UNAVAILABLE
  fill subscriber too slow ──> stream ends with DATA_LOSS and a message
                               telling you to resubscribe and reconcile
```

A rejected order is not an error. It is an answer.

POV is deliberately not here. It is implemented and tested in `algo-core`, but
the simulated venue publishes no per-order volume feed for a participation rate
to track, so exposing it would mean shipping a control that cannot work. It is
reachable from the library and from Python.

---

## The desktop client

An egui application that is a thin client and nothing more. Every number on
screen came from a gRPC reply: order state, child orders, positions with their
mark prices, streamed fills, the risk halt.

It offers the three algorithms the API actually accepts, with the parameters each
one takes, and it sends the one you selected. That sounds like a low bar. The
previous version showed four radio buttons, printed your choice to the terminal,
and sent TWAP regardless.

Form parsing and every unit conversion are pure functions with tests. That is
how a bug like sending `50000` when the engine wanted `5000000000` ticks — a
price 100 000 times too small — becomes a failing test rather than a confusing
afternoon.

---

## Python bindings

A PyO3 extension built with maturin, in a mixed layout: the Python package
`algo_exec_py` ships with the compiled module inside it as `algo_exec_py._native`.

```python
import algo_exec_py as ae

# (target_time_ns, qty) per child order
ae.twap_schedule(start_ns=0, end_ns=10_000_000_000, total_qty=1_000, num_slices=10)
ae.twap_schedule_randomized(0, 10_000_000_000, 1_000, 10, seed=42)

profile = ae.volume_profile_from_bars(bars, start_offset_ns, duration_ns, num_slices)
ae.vwap_schedule(0, 10_000_000_000, 1_000, profile)

# dict with children, scheduled_qty, shortfall_qty
ae.pov_schedule(0, 1_000, 10_000, 1_000, [(1, 500), (2, 500)])

# dict with children, holdings, kappa, expected_cost, cost_variance
ae.almgren_chriss_schedule(
    start_ns=0, end_ns=3_600_000_000_000, total_qty=100_000, num_slices=12,
    risk_aversion=1e-3, volatility=0.02, temporary_impact=0.5, permanent_impact=1e-6,
)
```

Which exception you get depends on how the argument is wrong, because PyO3
converts to Rust types before any of my code runs. The wrong type raises
`TypeError`. A negative or too-large integer raises `OverflowError`. A value my
validation rejects raises `ValueError` with the Rust message. The docstring used
to say everything raised `ValueError`, which was simply false, and there are now
tests pinning all three.

`requires-python` is `>=3.9,<3.14`. PyO3 0.22 supports 3.13 at the newest, and
without the upper bound `pip` on 3.14 happily starts a build that then fails in
the compiler with something unhelpful.

---

## How I know it works

Test count is close to meaningless on its own. What I care about is whether a
test would *fail* if the code were wrong. Five kinds of checking, roughly in
order of how much I trust them:

**A differential test against a deliberately naive model.** I wrote a second
order book for clarity rather than speed — scan every level, no index, no cached
totals — and then replay up to 79 random operations against both, comparing every
execution report, every cancel, full depth on both sides, the order count and the
invariants after each step. 512 random sequences per run. If my fast version and
my obvious version disagree anywhere, one of them is wrong.

**Property tests** for things that must hold for every input, not just the
examples I thought of: the accounting identity, schedule totals, monotonicity,
the POV participation cap, prefix invariance, and first-order optimality of the
Almgren–Chriss trajectory.

**Known-answer tests**, where I can get the right answer from somewhere other
than my own code: the first output of the reference `splitmix64.c`, closed-form
cost and variance computed by hand, an exact round-trip P&L, and a pinned
randomized schedule checked against an independent Python implementation of the
same generator.

**Mutation testing**, which is the one that changed my mind about my own test
suite. I broke the code in 20 specific ways — each a plausible mistake — and
checked that the suite failed for every one:

```text
  mutation                                        result
  ─────────────────────────────────────────────── ──────
  draw from one extra nanosecond                  killed
  skip the draw for empty slices                  killed
  modulo instead of multiply-shift reduction      killed
  assume slice widths are equal                   killed
  drop the monotonicity clamp                     killed
  round ties to even instead of away from zero    killed
  stop truncating the VWAP curve                  killed
  remove the MAX_SLICES guard                     killed
  shift the midnight boundary by one              killed
  treat small κT as exactly linear                killed
  evaluate the sinh ratio naively                 killed
  use acosh(1+y) for κ                            killed
  accept η̃ = 0                                    killed
  ... plus a revert of each bug I fixed           killed
  ─────────────────────────────────────────────── ──────
  20 of 20
```

Before that exercise, 14 of those mutations went completely unnoticed. The tests
passed on broken code. That is the gap between "I have tests" and "my tests
check something".

**An end-to-end test over a real socket** that submits an order, streams fills,
reads status and positions, and checks that a business rejection and a malformed
request come back differently.

---

## Results

174 Rust tests, which have to pass in debug *and* release, with
`clippy --workspace --all-targets -- -D warnings` clean. Debug keeps
integer-overflow checks on; release catches things that only appear optimised.

| Crate | Unit | Doc | Integration |
| --- | --- | --- | --- |
| `orderbook` | 38 | 1 | 1 (differential, 512 sequences) |
| `algo-core` | 58 | 5 | — |
| `engine` | 21 | 1 | 18 (17 scenarios, 1 end-to-end over gRPC) |
| `api` | 5 | 1 | — |
| `config` | 4 | — | — |
| `python-bindings` | 2 | — | — |
| `trader-gui` | 19 | — | — |
| **Total** | **147** | **8** | **19** |

Plus 34 Python test functions, 67 cases once parametrised.

Benchmarks from Criterion on the `bench` profile (release with LTO), on my
laptop. These measure book operations in one process on one thread. They are not
an end-to-end latency figure and I am not claiming one.

| What | Levels per side | Time |
| --- | --- | --- |
| Insert a passive limit order, then cancel it | 10 | 69.5 ns |
| Insert a passive limit order, then cancel it | 1 000 | 88.6 ns |
| IOC takes the best ask, then a passive order refills it | 10 | 131.9 ns |
| IOC takes the best ask, then a passive order refills it | 1 000 | 152.9 ns |
| Read best bid and best ask | 1 000 | 4.6 ns |
| 100 000 mixed operations | 100 | 11.1 ms → 9.0 M ops/s |

The mixed workload took some thought. My first version generated random
operations blind, which meant cancels often named orders that had already
filled, and the "crossing" IOC orders were priced at a fixed offset so about half
of them crossed nothing and traded nothing. Worse, the book grew about 29× during
the run, so the per-operation number was an average over a book that kept
changing size — run it for twice as long and you get a different answer.

It now generates against a shadow copy of the book, so cancels name orders that
are really resting and every IOC is priced through the live touch. The add/cancel
choice keeps the resting count inside a band, and the benchmark asserts the book
ends near the size it started at. With seed 7: 48 250 passive limit orders,
36 830 cancels, 14 920 IOC orders producing 20 956 fills, resting count 800 → 899.

---

## What I got wrong

I am listing these because the interesting part of the project is the second
pass, not the first. All of them are fixed.

### Mathematics and logic

| Where | What was wrong |
| --- | --- |
| `is.rs` | `κ` was rejected as overflowing for perfectly finite inputs — `√(y(y+2))` overflowed, so a schedule with `κ ≈ 1.55` was refused |
| `is.rs` | `cost_variance` came back NaN for valid inputs, from `inf × 0` when σ² overflowed and the trajectory was empty |
| `is.rs` | the cost function accepted non-finite holdings and returned NaN instead of an error |
| `is.rs` | `λ = 0` did not actually give TWAP's child quantities — 383 of 7 956 sizes disagreed |
| `book.rs` | `check_invariants` returned "fine" for a book with the same id resting at two levels |
| `types.rs` | the tick range accepted an out-of-range negative price while rejecting its positive mirror |
| `vwap.rs` | above 2^53 units, a trailing zero-volume slice was handed the leftover quantity |

### The client

| What was wrong |
| --- |
| limit prices were sent 100 000× too small — whole units where the engine wanted ticks |
| the algorithm selector was decorative: it printed your choice and always sent TWAP |
| P&L was fabricated. `avg_price` was the literal `100.0`, and sell P&L was `(100.0 − 99.0) × filled_qty`. Those invented numbers fed the Sharpe ratio, the drawdown and the equity curve on the analytics page |
| the news feed was generated from price deltas — "BTC surges 0.63% amid strong buying pressure" was written by a `format!` call |
| "Adaptive TWAP" fabricated filled child orders when the engine wasn't connected |
| submitted orders never appeared in the table at all: the poll read ids from the table, and nothing ever put them there |

### Three crates I deleted rather than fixed

`backtesting` stored a commission rate and never charged it, executed signals at
the same bar close that generated them, counted a short position as *adding* to
equity, and built its equity curve from trade P&L so that it disagreed with the
portfolio it was describing. `analytics` stamped the first equity point at
timestamp 0, so annualised return divided by about 55 years for any real
timestamp, and annualised per-trade returns by √252 as though every trade were a
day. `indicators` had no known-answer test anywhere — the RSI test asserted only
that values sat between 0 and 100, which a wrong RSI passes comfortably.

Nothing in the execution core depended on any of them. A backtester that flatters
itself is worse than no backtester, so they went.

---

## What it doesn't do

The things I would ask about first, if someone showed me this:

- **The simulated venue's price never moves.** A market maker quotes a ladder
  around a fixed reference price; fills eat that depth and the next tick restores
  it. So execution cost measured here is the scheduler walking a static book. It
  is not market impact. Which is worth sitting with, because managing impact is
  the entire reason these algorithms exist — the machinery and the measurement
  are real, the market model is not.
- **The Almgren–Chriss parameters are supplied, not estimated.** σ, η and γ come
  from you. The schedule is optimal given those numbers; calibrating them from
  market data is a different project.
- **No exchange connectivity.** No FIX, no venue adapters, no market data feed.
- **No backtester**, no commissions, no fees, no borrow costs, no latency model,
  no queue-position modelling beyond arrival order.
- **No self-trade prevention.** The book has no concept of who owns an order.
- **No authentication or TLS** on the API. The shipped config binds to loopback
  for that reason.
- **Nothing is persisted.** Engine state lives in memory for the life of the
  process, so this is a session, not a service.
- **POV is not exposed through the engine API**, for the reason given earlier.
- **The kill switch stops new orders and cancels working ones.** It does not
  liquidate what you are already holding.
- **Benchmarks are one machine, in one process.** I make no throughput claim for
  the engine or the API as a whole, because I have not measured one.

---

## Running it

```bash
git clone https://github.com/shaunak-batra/RustAlgoExecutionSystem.git
cd RustAlgoExecutionSystem

cargo build --release
cargo test --workspace            # 174 tests
```

`protoc` is vendored, so a clean clone builds with no system protobuf install and
no generated files checked in.

```bash
cargo run --release --bin engine                      # uses config/default.toml
cargo run --release --bin engine -- --config my.toml
cargo run --release --bin trader-gui                  # connects to 127.0.0.1:50051
```

Set `RUST_LOG=debug` for more engine logging. Ctrl-C shuts it down, draining
in-flight commands first.

The full check, which is also what CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo bench -p orderbook
```

Python:

```bash
pip install -r python/requirements.txt
pip install ./python
pytest python/tests -q           # 67 cases
```

Configuration lives in `config/default.toml`. Unknown keys are rejected, so a
typo is an error rather than a silent default, and every value is range-checked
with the offending field named:

```toml
[engine]
tick_interval_ms = 50
command_buffer = 1024
fill_buffer = 4096
retained_finished_orders = 10000

[api]
listen_addr = "127.0.0.1:50051"   # no auth, so loopback

[risk]
max_order_qty = 100000
max_order_notional = 10000000.0
max_position_qty = 500000
max_gross_notional = 50000000.0
price_collar_bps = 500
max_loss = 250000.0

[[venue.symbols]]
symbol = "BTC-USD"
reference_price = 65000.0
levels = 20
qty_per_level = 5
level_spacing_bps = 1
```

Three symbols ship configured: `BTC-USD`, `ETH-USD` and `SIM-EQ`. Money is
written in price units and converted to integer ticks by the engine.

---

## References

Almgren, R. and Chriss, N. (2000). "Optimal execution of portfolio
transactions." *Journal of Risk* 3(2), 5–39. The implementation shortfall
scheduler follows the discrete linear-impact model from this paper. I worked
through the derivation rather than copying the result, which is how I found that
my `λ = 0` case disagreed with TWAP.

Steele, G., Lea, D. and Flood, C. (2014). "Fast splittable pseudorandom number
generators." The `splitmix64` variant used for randomized release times.

---

## Licence

MIT. See [LICENSE](LICENSE).
