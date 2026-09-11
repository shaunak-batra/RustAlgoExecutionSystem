//! Order book benchmarks.
//!
//! The per-operation benchmarks restore the book to the same shape after every
//! iteration, so the measured cost does not drift as the benchmark runs. The
//! mixed workload replays a fixed, seeded stream of operations on a fresh copy
//! of the same starting book each iteration.

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use orderbook::{Order, OrderBook, OrderId, Price, Quantity, Side, TimeInForce, Timestamp};
use rand::{rngs::StdRng, Rng, SeedableRng};

/// Mid price shared by all benchmarks (100.00000).
const MID: i64 = 10_000_000;
const ORDERS_PER_LEVEL: u64 = 4;
const LOT: u64 = 10;

/// Book with `levels` price levels per side, one tick apart around `MID`, and
/// `ORDERS_PER_LEVEL` resting orders at each level. Returns the next unused id.
fn book_with_depth(levels: i64) -> (OrderBook, u64) {
    let mut book = OrderBook::new("BENCH".to_string());
    let mut id = 1u64;
    for level in 1..=levels {
        for _ in 0..ORDERS_PER_LEVEL {
            for (side, price) in [(Side::Buy, MID - level), (Side::Sell, MID + level)] {
                let order = Order::limit(
                    OrderId(id),
                    side,
                    Price(price),
                    Quantity(LOT),
                    Timestamp(id),
                );
                book.submit_order(order).expect("valid order");
                id += 1;
            }
        }
    }
    (book, id)
}

/// Insert a passive limit order into an existing level, then cancel it.
fn bench_insert_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_then_cancel_passive");
    for levels in [10i64, 1_000] {
        let (mut book, id) = book_with_depth(levels);
        let order = Order::limit(
            OrderId(id),
            Side::Buy,
            Price(MID - levels / 2),
            Quantity(LOT),
            Timestamp(0),
        );
        group.bench_function(BenchmarkId::new("levels_per_side", levels), |b| {
            b.iter(|| {
                book.submit_order(black_box(order)).expect("valid order");
                black_box(book.cancel_order(black_box(order.id)));
            })
        });
    }
    group.finish();
}

/// Aggressive IOC that fully fills the first order at the best ask, followed by
/// a passive sell that restores the level (the queue rotates by one order).
fn bench_take_and_replenish(c: &mut Criterion) {
    let mut group = c.benchmark_group("take_best_ask_then_replenish");
    for levels in [10i64, 1_000] {
        let (mut book, mut next_id) = book_with_depth(levels);
        let best_ask = Price(MID + 1);
        group.bench_function(BenchmarkId::new("levels_per_side", levels), |b| {
            b.iter(|| {
                let taker = Order::limit(
                    OrderId(next_id),
                    Side::Buy,
                    best_ask,
                    Quantity(LOT),
                    Timestamp(0),
                )
                .with_tif(TimeInForce::IOC);
                let maker = Order::limit(
                    OrderId(next_id + 1),
                    Side::Sell,
                    best_ask,
                    Quantity(LOT),
                    Timestamp(0),
                );
                next_id += 2;
                black_box(book.submit_order(black_box(taker)).expect("valid order"));
                book.submit_order(black_box(maker)).expect("valid order");
            })
        });
    }
    group.finish();
}

fn bench_top_of_book(c: &mut Criterion) {
    let (book, _) = book_with_depth(1_000);
    c.bench_function("best_bid_ask/levels_per_side/1000", |b| {
        b.iter(|| black_box((book.best_bid(), book.best_ask())))
    });
}

#[derive(Clone, Copy)]
enum Op {
    Submit(Order),
    Cancel(OrderId),
}

/// Resting order count the mixed workload is held at, and the width of the band
/// around it. The starting book holds exactly `TARGET_RESTING` orders.
const TARGET_RESTING: usize = 800;
const BAND: usize = 100;

/// A generated operation stream, with the figures needed to read the result.
struct Workload {
    ops: Vec<Op>,
    resting_at_start: usize,
    resting_at_end: usize,
    limits: usize,
    cancels: usize,
    iocs: usize,
    fills: usize,
}

/// Seeded operation stream, generated against a shadow copy of the starting
/// book so that every operation is one the book can actually act on:
/// - passive limit orders within 50 ticks of mid, which never cross;
/// - cancels of an order that is still resting at that point in the stream;
/// - 15% IOC orders priced 0 to 2 ticks through the opposite touch, so each one
///   crosses and trades rather than resting or trading nothing.
///
/// The add/cancel choice keeps the resting count inside `TARGET_RESTING ± BAND`.
/// Without that the book grows as the stream replays, and the per-operation
/// figure is an average over a book of changing size rather than a steady-state
/// cost that can be compared between runs.
///
/// With seed 7 and 100_000 operations the stream is 48_250 passive limit orders,
/// 36_830 cancels and 14_920 IOC orders producing 20_956 fills, and the resting
/// count goes from 800 to 899. [`bench_mixed_workload`] prints these figures and
/// asserts the start and end sizes stay within `BAND` of each other.
fn mixed_workload(n: usize, start: &OrderBook, first_id: u64, seed: u64) -> Workload {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut shadow = start.clone();
    let resting_at_start = shadow.order_count();
    // Ids that may still be resting; entries for filled orders are dropped lazily.
    let mut pool: Vec<OrderId> = (1..first_id).map(OrderId).collect();
    let mut ops = Vec::with_capacity(n);
    let mut id = first_id;
    let (mut limits, mut cancels, mut iocs, mut fills) = (0, 0, 0, 0);

    while ops.len() < n {
        let side = if rng.gen_bool(0.5) {
            Side::Buy
        } else {
            Side::Sell
        };
        let qty = Quantity(rng.gen_range(1..=100));
        let roll = rng.gen_range(0..100);
        let resting = shadow.order_count();
        let mut emitted = false;

        if roll >= 85 {
            // Price through the opposite touch so the order always trades.
            let through = rng.gen_range(0..=2);
            let limit = match side {
                Side::Buy => shadow.best_ask().map(|ask| ask.ticks() + through),
                Side::Sell => shadow.best_bid().map(|bid| bid.ticks() - through),
            };
            if let Some(ticks) = limit {
                let order = Order::limit(OrderId(id), side, Price(ticks), qty, Timestamp(id))
                    .with_tif(TimeInForce::IOC);
                id += 1;
                let result = shadow.submit_order(order).expect("valid order");
                assert!(
                    !result.fills.is_empty(),
                    "an IOC through the touch must trade"
                );
                fills += result.fills.len();
                ops.push(Op::Submit(order));
                iocs += 1;
                emitted = true;
            }
        } else if resting >= TARGET_RESTING + BAND
            || (resting > TARGET_RESTING - BAND && roll >= 55)
        {
            while !pool.is_empty() {
                let pick = rng.gen_range(0..pool.len());
                let candidate = pool.swap_remove(pick);
                if shadow.contains_order(candidate) {
                    shadow.cancel_order(candidate).expect("order rests");
                    ops.push(Op::Cancel(candidate));
                    cancels += 1;
                    emitted = true;
                    break;
                }
            }
        }

        // A passive limit order, and the fallback when there was nothing to take
        // or nothing left to cancel.
        if !emitted {
            let offset = rng.gen_range(1..=50);
            let ticks = match side {
                Side::Buy => MID - offset,
                Side::Sell => MID + offset,
            };
            let order = Order::limit(OrderId(id), side, Price(ticks), qty, Timestamp(id));
            id += 1;
            let result = shadow.submit_order(order).expect("valid order");
            assert!(result.fills.is_empty(), "a passive order must not cross");
            pool.push(order.id);
            ops.push(Op::Submit(order));
            limits += 1;
        }
    }

    Workload {
        ops,
        resting_at_start,
        resting_at_end: shadow.order_count(),
        limits,
        cancels,
        iocs,
        fills,
    }
}

fn bench_mixed_workload(c: &mut Criterion) {
    const OPS: usize = 100_000;
    let (start, first_id) = book_with_depth(100);
    let workload = mixed_workload(OPS, &start, first_id, 7);

    // If the book does not end near the size it started at, the throughput
    // figure below describes a book that grew rather than a steady state.
    assert!(
        workload.resting_at_end.abs_diff(workload.resting_at_start) <= BAND,
        "not a steady state: {} resting orders at the start, {} at the end",
        workload.resting_at_start,
        workload.resting_at_end
    );
    println!(
        "mixed workload: {} limit, {} cancel, {} IOC ({} fills); \
         resting {} -> {}",
        workload.limits,
        workload.cancels,
        workload.iocs,
        workload.fills,
        workload.resting_at_start,
        workload.resting_at_end
    );

    let ops = &workload.ops;
    let mut group = c.benchmark_group("mixed_workload");
    group.throughput(Throughput::Elements(OPS as u64));
    group.sample_size(20);
    group.bench_function("100k_ops_steady_state", |b| {
        b.iter_batched(
            || start.clone(),
            |mut book| {
                for op in ops {
                    match *op {
                        Op::Submit(order) => {
                            black_box(book.submit_order(order).expect("valid order"));
                        }
                        Op::Cancel(id) => {
                            black_box(book.cancel_order(id));
                        }
                    }
                }
                book
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_insert_cancel,
    bench_take_and_replenish,
    bench_top_of_book,
    bench_mixed_workload
);
criterion_main!(benches);
