//! Differential test: the order book must behave exactly like a deliberately
//! naive reference book (a flat list, linear scans, no index, no cached level
//! totals) on random sequences of submits and cancels.
//!
//! After every step the test compares the execution report or cancel result,
//! the full depth of both sides, and the resting order count, and runs the
//! book's own invariant checker.

use orderbook::{
    Fill, Order, OrderBook, OrderError, OrderId, OrderOutcome, Price, Quantity, Side, SubmitResult,
    TimeInForce, Timestamp,
};
use proptest::prelude::*;
use std::collections::BTreeMap;

/// Resting orders tagged with an arrival sequence number.
#[derive(Default)]
struct ReferenceBook {
    resting: Vec<(u64, Order)>,
    next_seq: u64,
}

impl ReferenceBook {
    fn submit(&mut self, mut order: Order) -> Result<SubmitResult, OrderError> {
        if order.qty.value() == 0 {
            return Err(OrderError::ZeroQuantity);
        }
        match order.limit_price {
            Some(price) if price.ticks() <= 0 => {
                return Err(OrderError::NonPositivePrice(price.ticks()))
            }
            None if order.tif == TimeInForce::GTC => return Err(OrderError::MarketOrderCannotRest),
            _ => {}
        }
        // (LevelQuantityOverflow is unreachable with the small quantities generated here.)
        if self
            .resting
            .iter()
            .any(|(_, resting)| resting.id == order.id)
        {
            return Err(OrderError::DuplicateOrderId(order.id.value()));
        }

        let side = order.side;
        let limit = order.limit_price;
        let will_trade_with = |resting: &Order| {
            let price = resting
                .limit_price
                .expect("resting orders have a price")
                .ticks();
            resting.side != side
                && match (side, limit) {
                    (_, None) => true,
                    (Side::Buy, Some(limit)) => price <= limit.ticks(),
                    (Side::Sell, Some(limit)) => price >= limit.ticks(),
                }
        };

        // Best price for the incoming order first, then arrival order.
        let mut candidates: Vec<usize> = (0..self.resting.len())
            .filter(|&i| will_trade_with(&self.resting[i].1))
            .collect();
        candidates.sort_by_key(|&i| {
            let (seq, resting) = &self.resting[i];
            let ticks = resting
                .limit_price
                .expect("resting orders have a price")
                .ticks();
            let price_rank = match side {
                Side::Buy => ticks,
                Side::Sell => -ticks,
            };
            (price_rank, *seq)
        });

        let available: u64 = candidates
            .iter()
            .map(|&i| self.resting[i].1.qty.value())
            .sum();
        if order.tif == TimeInForce::FOK && available < order.qty.value() {
            return Ok(SubmitResult {
                fills: Vec::new(),
                filled_qty: Quantity::ZERO,
                outcome: OrderOutcome::Killed,
            });
        }

        let requested = order.qty.value();
        let mut fills = Vec::new();
        for i in candidates {
            if order.qty.value() == 0 {
                break;
            }
            let maker = &mut self.resting[i].1;
            let qty = order.qty.value().min(maker.qty.value());
            order.qty = Quantity::new(order.qty.value() - qty);
            maker.qty = Quantity::new(maker.qty.value() - qty);
            fills.push(Fill {
                taker_order_id: order.id,
                maker_order_id: maker.id,
                taker_side: side,
                price: maker.limit_price.expect("resting orders have a price"),
                qty: Quantity::new(qty),
                timestamp: order.timestamp,
            });
        }
        self.resting.retain(|(_, resting)| resting.qty.value() > 0);

        let filled_qty = Quantity::new(requested - order.qty.value());
        let outcome = if order.qty.value() == 0 {
            OrderOutcome::Filled
        } else if order.tif == TimeInForce::GTC {
            let remaining = order.qty;
            self.resting.push((self.next_seq, order));
            self.next_seq += 1;
            OrderOutcome::Resting { remaining }
        } else {
            OrderOutcome::Cancelled {
                remaining: order.qty,
            }
        };

        Ok(SubmitResult {
            fills,
            filled_qty,
            outcome,
        })
    }

    fn cancel(&mut self, id: OrderId) -> Option<Order> {
        let position = self.resting.iter().position(|(_, order)| order.id == id)?;
        Some(self.resting.remove(position).1)
    }

    /// Aggregated `(price, quantity)` levels, best price first.
    fn depth(&self, side: Side) -> Vec<(Price, Quantity)> {
        let mut levels: BTreeMap<i64, u64> = BTreeMap::new();
        for (_, order) in self.resting.iter().filter(|(_, order)| order.side == side) {
            let ticks = order
                .limit_price
                .expect("resting orders have a price")
                .ticks();
            *levels.entry(ticks).or_default() += order.qty.value();
        }
        let levels = levels
            .into_iter()
            .map(|(ticks, qty)| (Price::new(ticks), Quantity::new(qty)));
        match side {
            Side::Buy => levels.rev().collect(),
            Side::Sell => levels.collect(),
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    Submit {
        side: Side,
        price: Option<i64>,
        qty: u64,
        tif: TimeInForce,
        /// Reuse an earlier id instead of a fresh one.
        reuse_id: Option<usize>,
    },
    Cancel {
        pick: usize,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let side = prop_oneof![Just(Side::Buy), Just(Side::Sell)];
    // A narrow price band so orders cross often; occasional market orders and invalid prices.
    let price = prop_oneof![
        18 => (95i64..=105).prop_map(Some),
        2 => Just(None),
        1 => Just(Some(0)),
    ];
    let qty = prop_oneof![19 => 1u64..=20, 1 => Just(0u64)];
    let tif = prop_oneof![
        3 => Just(TimeInForce::GTC),
        1 => Just(TimeInForce::IOC),
        1 => Just(TimeInForce::FOK),
    ];
    let reuse_id = prop_oneof![9 => Just(None), 1 => any::<usize>().prop_map(Some)];

    prop_oneof![
        4 => (side, price, qty, tif, reuse_id).prop_map(|(side, price, qty, tif, reuse_id)| {
            Op::Submit { side, price, qty, tif, reuse_id }
        }),
        1 => any::<usize>().prop_map(|pick| Op::Cancel { pick }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn order_book_matches_reference_model(ops in proptest::collection::vec(op(), 1..80)) {
        let mut book = OrderBook::new("PROP".to_string());
        let mut model = ReferenceBook::default();
        let mut used_ids: Vec<OrderId> = Vec::new();
        let mut next_id = 1u64;

        for (step, op) in ops.into_iter().enumerate() {
            match op {
                Op::Submit { side, price, qty, tif, reuse_id } => {
                    let id = match reuse_id {
                        Some(pick) if !used_ids.is_empty() => used_ids[pick % used_ids.len()],
                        _ => {
                            let id = OrderId::new(next_id);
                            next_id += 1;
                            used_ids.push(id);
                            id
                        }
                    };
                    let order = Order {
                        id,
                        side,
                        limit_price: price.map(Price::new),
                        qty: Quantity::new(qty),
                        timestamp: Timestamp::new(step as u64),
                        tif,
                    };

                    let expected = model.submit(order);
                    let actual = book.submit_order(order);
                    prop_assert_eq!(&actual, &expected, "step {}: {:?}", step, order);

                    // Properties that hold regardless of the model.
                    if let Ok(result) = &actual {
                        let traded: u64 = result.fills.iter().map(|fill| fill.qty.value()).sum();
                        prop_assert_eq!(traded, result.filled_qty.value());
                        prop_assert!(traded <= qty);
                        if let Some(limit) = order.limit_price {
                            for fill in &result.fills {
                                match side {
                                    Side::Buy => prop_assert!(fill.price <= limit),
                                    Side::Sell => prop_assert!(fill.price >= limit),
                                }
                            }
                        }
                    }
                }
                Op::Cancel { pick } => {
                    let id = if used_ids.is_empty() {
                        OrderId::new(u64::MAX)
                    } else {
                        used_ids[pick % used_ids.len()]
                    };
                    prop_assert_eq!(book.cancel_order(id), model.cancel(id), "step {}: cancel {:?}", step, id);
                }
            }

            prop_assert_eq!(book.bid_levels(usize::MAX), model.depth(Side::Buy), "bids after step {}", step);
            prop_assert_eq!(book.ask_levels(usize::MAX), model.depth(Side::Sell), "asks after step {}", step);
            prop_assert_eq!(book.order_count(), model.resting.len(), "order count after step {}", step);
            if let Err(violation) = book.check_invariants() {
                prop_assert!(false, "invariant violated after step {}: {}", step, violation);
            }
        }
    }
}
