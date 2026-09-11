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

/// Seeded operation stream: 55% passive limit orders within 50 ticks of mid,
/// 30% cancels of previously submitted ids (some already filled), and 15%
/// IOC orders that cross up to 5 ticks into the other side.
fn mixed_workload(n: usize, first_id: u64, seed: u64) -> Vec<Op> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut ops = Vec::with_capacity(n);
    let mut submitted: Vec<OrderId> = Vec::new();
    let mut id = first_id;

    while ops.len() < n {
        let roll = rng.gen_range(0..100);
        let side = if rng.gen_bool(0.5) {
            Side::Buy
        } else {
            Side::Sell
        };
        let qty = Quantity(rng.gen_range(1..=100));

        if roll < 55 {
            let offset = rng.gen_range(1..=50);
            let price = match side {
                Side::Buy => MID - offset,
                Side::Sell => MID + offset,
            };
            ops.push(Op::Submit(Order::limit(
                OrderId(id),
                side,
                Price(price),
                qty,
                Timestamp(id),
            )));
            submitted.push(OrderId(id));
            id += 1;
        } else if roll < 85 && !submitted.is_empty() {
            let pick = rng.gen_range(0..submitted.len());
            ops.push(Op::Cancel(submitted.swap_remove(pick)));
        } else {
            let price = match side {
                Side::Buy => MID + 5,
                Side::Sell => MID - 5,
            };
            let order = Order::limit(OrderId(id), side, Price(price), qty, Timestamp(id))
                .with_tif(TimeInForce::IOC);
            ops.push(Op::Submit(order));
            id += 1;
        }
    }
    ops
}

fn bench_mixed_workload(c: &mut Criterion) {
    const OPS: usize = 100_000;
    let (start, first_id) = book_with_depth(100);
    let ops = mixed_workload(OPS, first_id, 7);

    let mut group = c.benchmark_group("mixed_workload");
    group.throughput(Throughput::Elements(OPS as u64));
    group.sample_size(20);
    group.bench_function("100k_ops_55limit_30cancel_15ioc", |b| {
        b.iter_batched(
            || start.clone(),
            |mut book| {
                for op in &ops {
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
