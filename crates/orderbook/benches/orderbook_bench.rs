use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use orderbook::{Order, OrderBook, OrderId, Price, Quantity, Side, Timestamp};

fn create_order(id: u64, side: Side, price: f64, qty: u64) -> Order {
    Order::new(
        OrderId::new(id),
        side,
        Price::from_f64(price),
        Quantity::new(qty),
        Timestamp::now_nanos(),
    )
}

fn bench_order_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("order_insertion");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let mut book = OrderBook::new("BTC-USD".to_string());

                // Insert buy orders
                for i in 0..size {
                    let order = create_order(i as u64, Side::Buy, 100.0 - i as f64 * 0.01, 10);
                    book.insert_order(black_box(order));
                }

                // Insert sell orders
                for i in 0..size {
                    let order =
                        create_order((size + i) as u64, Side::Sell, 101.0 + i as f64 * 0.01, 10);
                    book.insert_order(black_box(order));
                }
            });
        });
    }

    group.finish();
}

fn bench_marketable_order(c: &mut Criterion) {
    c.bench_function("marketable_order_full_fill", |b| {
        b.iter(|| {
            let mut book = OrderBook::new("BTC-USD".to_string());

            // Add resting sell order
            let sell_order = create_order(1, Side::Sell, 100.0, 100);
            book.insert_order(sell_order);

            // Aggressive buy order (should immediately fill)
            let buy_order = create_order(2, Side::Buy, 100.0, 100);
            let fills = book.insert_order(black_box(buy_order));

            black_box(fills);
        });
    });
}

fn bench_partial_fill(c: &mut Criterion) {
    c.bench_function("marketable_order_partial_fill", |b| {
        b.iter(|| {
            let mut book = OrderBook::new("BTC-USD".to_string());

            // Add resting sell order for 100
            let sell_order = create_order(1, Side::Sell, 100.0, 100);
            book.insert_order(sell_order);

            // Aggressive buy order for 50 (partial fill)
            let buy_order = create_order(2, Side::Buy, 100.0, 50);
            let fills = book.insert_order(black_box(buy_order));

            black_box(fills);
        });
    });
}

fn bench_multiple_levels(c: &mut Criterion) {
    c.bench_function("match_across_multiple_levels", |b| {
        b.iter(|| {
            let mut book = OrderBook::new("BTC-USD".to_string());

            // Add multiple resting sell orders at different prices
            for i in 0..10 {
                let order = create_order(i, Side::Sell, 100.0 + i as f64 * 0.1, 10);
                book.insert_order(order);
            }

            // Large aggressive buy that crosses multiple levels
            let buy_order = create_order(100, Side::Buy, 105.0, 100);
            let fills = book.insert_order(black_box(buy_order));

            black_box(fills);
        });
    });
}

fn bench_best_bid_ask(c: &mut Criterion) {
    let mut book = OrderBook::new("BTC-USD".to_string());

    // Populate book with orders
    for i in 0..100 {
        book.insert_order(create_order(i, Side::Buy, 99.0 - i as f64 * 0.01, 10));
        book.insert_order(create_order(
            i + 100,
            Side::Sell,
            101.0 + i as f64 * 0.01,
            10,
        ));
    }

    c.bench_function("best_bid_ask_query", |b| {
        b.iter(|| {
            let bid = book.best_bid();
            let ask = book.best_ask();
            black_box((bid, ask));
        });
    });
}

fn bench_price_conversions(c: &mut Criterion) {
    c.bench_function("price_from_f64", |b| {
        b.iter(|| {
            let price = Price::from_f64(black_box(12345.6789));
            black_box(price);
        });
    });

    c.bench_function("price_to_f64", |b| {
        let price = Price::from_f64(12345.6789);
        b.iter(|| {
            let value = price.as_f64();
            black_box(value);
        });
    });
}

criterion_group!(
    benches,
    bench_order_insertion,
    bench_marketable_order,
    bench_partial_fill,
    bench_multiple_levels,
    bench_best_bid_ask,
    bench_price_conversions
);
criterion_main!(benches);
