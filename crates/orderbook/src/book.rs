use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use thiserror::Error;

/// An order submitted to the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    /// Limit price. `None` makes this a market order, which must be IOC or FOK.
    pub limit_price: Option<Price>,
    pub qty: Quantity,
    /// Informational. Queue priority is arrival order, not this value.
    pub timestamp: Timestamp,
    pub tif: TimeInForce,
}

impl Order {
    /// Good-till-cancelled limit order.
    pub fn limit(
        id: OrderId,
        side: Side,
        price: Price,
        qty: Quantity,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            limit_price: Some(price),
            qty,
            timestamp,
            tif: TimeInForce::GTC,
        }
    }

    /// Market order: trades at the best available prices and cancels any
    /// unfilled remainder (IOC). Use [`Order::with_tif`] for a market FOK.
    pub fn market(id: OrderId, side: Side, qty: Quantity, timestamp: Timestamp) -> Self {
        Self {
            id,
            side,
            limit_price: None,
            qty,
            timestamp,
            tif: TimeInForce::IOC,
        }
    }

    pub fn with_tif(mut self, tif: TimeInForce) -> Self {
        self.tif = tif;
        self
    }

    /// Whether this order is willing to trade against a resting order at `price`.
    fn accepts(&self, price: Price) -> bool {
        match (self.side, self.limit_price) {
            (_, None) => true,
            (Side::Buy, Some(limit)) => price <= limit,
            (Side::Sell, Some(limit)) => price >= limit,
        }
    }
}

/// One match between an incoming (taker) order and a resting (maker) order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub taker_order_id: OrderId,
    pub maker_order_id: OrderId,
    /// Side of the taker; the maker is on the opposite side.
    pub taker_side: Side,
    /// Execution price: always the maker's resting price.
    pub price: Price,
    pub qty: Quantity,
    /// Timestamp of the taker order that caused the match.
    pub timestamp: Timestamp,
}

/// What happened to a submitted order after matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderOutcome {
    /// The full quantity traded.
    Filled,
    /// A GTC remainder is now resting in the book.
    Resting { remaining: Quantity },
    /// The unfilled remainder of an IOC order (limit or market) was cancelled,
    /// possibly after partial fills.
    Cancelled { remaining: Quantity },
    /// A FOK order could not trade its full quantity; nothing traded and the
    /// book is unchanged.
    Killed,
}

/// Execution report for a submitted order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitResult {
    /// Matches in execution order.
    pub fills: Vec<Fill>,
    /// Sum of `fills[i].qty`.
    pub filled_qty: Quantity,
    pub outcome: OrderOutcome,
}

/// Reasons an order is rejected before it touches the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OrderError {
    #[error("order quantity must be greater than zero")]
    ZeroQuantity,
    #[error("limit price must be positive, got {0} ticks")]
    NonPositivePrice(i64),
    #[error("market orders must be IOC or FOK")]
    MarketOrderCannotRest,
    #[error("order id {0} is already resting in the book")]
    DuplicateOrderId(u64),
    #[error("resting quantity at this price level would overflow u64")]
    LevelQuantityOverflow,
}

/// Orders resting at one price, in arrival order.
#[derive(Debug, Clone, Default)]
struct Level {
    /// Front has the highest time priority.
    orders: VecDeque<Order>,
    /// Sum of the resting quantities, maintained incrementally.
    total_qty: u64,
}

/// Single-symbol limit order book with price-time priority.
///
/// Matching rules:
/// - A trade executes at the resting (maker) order's price.
/// - The best price level fills first; within a level, earlier orders fill first.
/// - A partially filled resting order keeps its place in the queue.
///
/// Complexity, with `L` price levels on a side and `k` orders at one level.
/// The id index is a hash map, so any bound that touches it is expected and
/// amortized rather than worst case:
/// - [`submit_order`](Self::submit_order): `O(log L)` per price level touched,
///   plus expected amortized `O(1)` per resting order filled or rested
/// - [`cancel_order`](Self::cancel_order): expected `O(1)` index lookup,
///   `O(log L)` level lookup, `O(k)` scan within the level
/// - [`best_bid`](Self::best_bid) / [`best_ask`](Self::best_ask): `O(log L)`
///
/// Self-trade prevention is not modelled: the book has no notion of order owners.
#[derive(Debug, Clone)]
pub struct OrderBook {
    symbol: String,
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    /// Resting order id -> (side, price), so cancels do not scan the book.
    index: HashMap<OrderId, (Side, Price)>,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            index: HashMap::new(),
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Submits an order: matches it against the opposite side, then rests any
    /// GTC remainder at its limit price.
    ///
    /// The outcome depends only on the time in force; a market order (no limit
    /// price) follows the same rule as a limit order with the same one:
    /// - `GTC` limit: the unfilled remainder rests.
    /// - `IOC` (limit or market): trade what is available, cancel the rest.
    /// - `FOK` (limit or market): trade the full quantity immediately, or
    ///   nothing (the book is left unchanged).
    ///
    /// Invalid orders return `Err` and leave the book unchanged.
    pub fn submit_order(&mut self, order: Order) -> Result<SubmitResult, OrderError> {
        self.validate(&order)?;

        if order.tif == TimeInForce::FOK && self.executable_qty(&order) < order.qty.value() {
            return Ok(SubmitResult {
                fills: Vec::new(),
                filled_qty: Quantity::ZERO,
                outcome: OrderOutcome::Killed,
            });
        }

        let mut order = order;
        let requested = order.qty;
        let fills = self.match_against_book(&mut order);
        let filled_qty = Quantity::new(requested.value() - order.qty.value());

        let outcome = match (order.tif, order.limit_price) {
            _ if order.qty.is_zero() => OrderOutcome::Filled,
            (TimeInForce::GTC, Some(price)) => {
                let remaining = order.qty;
                self.rest(order, price);
                OrderOutcome::Resting { remaining }
            }
            // IOC and market remainders. (FOK is fully filled by construction,
            // and a GTC market order was rejected during validation.)
            _ => OrderOutcome::Cancelled {
                remaining: order.qty,
            },
        };

        Ok(SubmitResult {
            fills,
            filled_qty,
            outcome,
        })
    }

    /// Cancels a resting order and returns it with its remaining quantity.
    ///
    /// Returns `None` if no order with this id is resting: it never existed,
    /// was fully filled, or was already cancelled.
    pub fn cancel_order(&mut self, id: OrderId) -> Option<Order> {
        let (side, price) = self.index.remove(&id)?;
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = levels.get_mut(&price)?;
        let position = level.orders.iter().position(|order| order.id == id)?;
        let order = level.orders.remove(position)?;
        level.total_qty -= order.qty.value();
        if level.orders.is_empty() {
            levels.remove(&price);
        }
        Some(order)
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    /// Midpoint of the best bid and best ask, rounded down to a whole tick.
    pub fn mid(&self) -> Option<Price> {
        let (bid, ask) = (self.best_bid()?, self.best_ask()?);
        let sum = i128::from(bid.ticks()) + i128::from(ask.ticks());
        Some(Price::new((sum / 2) as i64))
    }

    pub fn spread(&self) -> Option<Price> {
        Some(Price::new(
            self.best_ask()?.ticks() - self.best_bid()?.ticks(),
        ))
    }

    /// Total resting bid quantity at `price`.
    pub fn bid_qty_at(&self, price: Price) -> Quantity {
        Quantity::new(self.level_qty(Side::Buy, price))
    }

    /// Total resting ask quantity at `price`.
    pub fn ask_qty_at(&self, price: Price) -> Quantity {
        Quantity::new(self.level_qty(Side::Sell, price))
    }

    /// Top `n` bid levels as `(price, total quantity)`, highest price first.
    pub fn bid_levels(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.bids
            .iter()
            .rev()
            .take(n)
            .map(|(price, level)| (*price, Quantity::new(level.total_qty)))
            .collect()
    }

    /// Top `n` ask levels as `(price, total quantity)`, lowest price first.
    pub fn ask_levels(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.asks
            .iter()
            .take(n)
            .map(|(price, level)| (*price, Quantity::new(level.total_qty)))
            .collect()
    }

    /// Number of resting orders on both sides.
    pub fn order_count(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains_order(&self, id: OrderId) -> bool {
        self.index.contains_key(&id)
    }

    /// A resting order with its current (remaining) quantity.
    pub fn resting_order(&self, id: OrderId) -> Option<&Order> {
        let (side, price) = self.index.get(&id)?;
        self.levels(*side)
            .get(price)?
            .orders
            .iter()
            .find(|order| order.id == id)
    }

    /// Checks every internal consistency rule of the book in expected `O(n)`:
    /// no empty levels, no id resting twice, cached level totals match their
    /// orders, the id index matches the resting orders exactly, resting orders
    /// are GTC limit orders on the right side and level, and the book is not
    /// crossed.
    ///
    /// Intended for tests and debugging; returns a description of the first
    /// violation found.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut resting = 0usize;
        // Ids must be unique across the whole book. Without this check a
        // duplicate id paired with a stale index entry keeps the counts below
        // matching, so the index comparison would not prove an exact match.
        let mut seen = HashSet::with_capacity(self.index.len());
        for (side, levels) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            for (&price, level) in levels {
                if level.orders.is_empty() {
                    return Err(format!("empty {side} level at {price}"));
                }
                let mut total = 0u64;
                for order in &level.orders {
                    if order.qty.is_zero() {
                        return Err(format!("{} rests with zero quantity", order.id));
                    }
                    if order.side != side || order.limit_price != Some(price) {
                        return Err(format!("{} rests at the wrong side or level", order.id));
                    }
                    if order.tif != TimeInForce::GTC {
                        return Err(format!("{} rests with {:?}", order.id, order.tif));
                    }
                    if !seen.insert(order.id) {
                        return Err(format!("{} rests twice", order.id));
                    }
                    if self.index.get(&order.id) != Some(&(side, price)) {
                        return Err(format!("index entry for {} is missing or stale", order.id));
                    }
                    total = total
                        .checked_add(order.qty.value())
                        .ok_or_else(|| format!("{side} level {price} total overflows"))?;
                    resting += 1;
                }
                if total != level.total_qty {
                    return Err(format!(
                        "{side} level {price} caches {} but holds {total}",
                        level.total_qty
                    ));
                }
            }
        }
        if resting != self.index.len() {
            return Err(format!(
                "index has {} entries for {resting} resting orders",
                self.index.len()
            ));
        }
        if let (Some(bid), Some(ask)) = (self.best_bid(), self.best_ask()) {
            if bid >= ask {
                return Err(format!("book is crossed: bid {bid} >= ask {ask}"));
            }
        }
        Ok(())
    }

    fn validate(&self, order: &Order) -> Result<(), OrderError> {
        if order.qty.is_zero() {
            return Err(OrderError::ZeroQuantity);
        }
        match order.limit_price {
            Some(price) if price.ticks() <= 0 => {
                return Err(OrderError::NonPositivePrice(price.ticks()));
            }
            Some(price) if order.tif == TimeInForce::GTC => {
                // The whole quantity may end up resting at this level.
                if self
                    .level_qty(order.side, price)
                    .checked_add(order.qty.value())
                    .is_none()
                {
                    return Err(OrderError::LevelQuantityOverflow);
                }
            }
            None if order.tif == TimeInForce::GTC => {
                return Err(OrderError::MarketOrderCannotRest);
            }
            _ => {}
        }
        if self.index.contains_key(&order.id) {
            return Err(OrderError::DuplicateOrderId(order.id.value()));
        }
        Ok(())
    }

    fn levels(&self, side: Side) -> &BTreeMap<Price, Level> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    fn level_qty(&self, side: Side, price: Price) -> u64 {
        self.levels(side)
            .get(&price)
            .map_or(0, |level| level.total_qty)
    }

    /// Quantity the order could trade right now, capped at the order's size.
    fn executable_qty(&self, order: &Order) -> u64 {
        fn sum<'a>(levels: impl Iterator<Item = (&'a Price, &'a Level)>, order: &Order) -> u64 {
            let mut available = 0u64;
            for (&price, level) in levels {
                if available >= order.qty.value() || !order.accepts(price) {
                    break;
                }
                available = available.saturating_add(level.total_qty);
            }
            available.min(order.qty.value())
        }

        match order.side {
            Side::Buy => sum(self.asks.iter(), order),
            Side::Sell => sum(self.bids.iter().rev(), order),
        }
    }

    /// Trades `taker` against the opposite side until it is filled or no
    /// acceptable price remains. Decrements `taker.qty` as it fills.
    fn match_against_book(&mut self, taker: &mut Order) -> Vec<Fill> {
        let mut fills = Vec::new();

        while !taker.qty.is_zero() {
            let best = match taker.side {
                Side::Buy => self.asks.first_entry(),
                Side::Sell => self.bids.last_entry(),
            };
            let Some(mut entry) = best else { break };
            let price = *entry.key();
            if !taker.accepts(price) {
                break;
            }

            let level = entry.get_mut();
            while !taker.qty.is_zero() {
                let Some(maker) = level.orders.front_mut() else {
                    break;
                };
                let qty = taker.qty.value().min(maker.qty.value());
                taker.qty = Quantity::new(taker.qty.value() - qty);
                maker.qty = Quantity::new(maker.qty.value() - qty);
                level.total_qty -= qty;
                fills.push(Fill {
                    taker_order_id: taker.id,
                    maker_order_id: maker.id,
                    taker_side: taker.side,
                    price,
                    qty: Quantity::new(qty),
                    timestamp: taker.timestamp,
                });
                if maker.qty.is_zero() {
                    if let Some(filled) = level.orders.pop_front() {
                        self.index.remove(&filled.id);
                    }
                }
            }

            if level.orders.is_empty() {
                entry.remove();
            }
        }

        fills
    }

    /// Appends `order` to the back of the queue at `price`.
    fn rest(&mut self, order: Order, price: Price) {
        let levels = match order.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = levels.entry(price).or_default();
        // Cannot overflow: `validate` checked this level's total against the
        // full order quantity, and matching only touches the opposite side.
        level.total_qty += order.qty.value();
        self.index.insert(order.id, (order.side, price));
        level.orders.push_back(order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(price: f64) -> Price {
        Price::from_f64(price)
    }

    fn limit(id: u64, side: Side, price: f64, qty: u64) -> Order {
        Order::limit(OrderId(id), side, px(price), Quantity(qty), Timestamp(id))
    }

    /// Submits a valid order and checks the book's invariants afterwards.
    fn submit(book: &mut OrderBook, order: Order) -> SubmitResult {
        let result = book.submit_order(order).expect("order should be valid");
        book.check_invariants().expect("invariants should hold");
        result
    }

    /// (taker id, maker id, price, qty) for each fill.
    fn trades(result: &SubmitResult) -> Vec<(u64, u64, Price, u64)> {
        result
            .fills
            .iter()
            .map(|f| (f.taker_order_id.0, f.maker_order_id.0, f.price, f.qty.0))
            .collect()
    }

    fn book() -> OrderBook {
        OrderBook::new("TEST".to_string())
    }

    #[test]
    fn empty_book_has_no_prices() {
        let book = book();
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.mid(), None);
        assert_eq!(book.spread(), None);
        assert!(book.is_empty());
    }

    #[test]
    fn non_crossing_limit_orders_rest() {
        let mut book = book();
        let bid = submit(&mut book, limit(1, Side::Buy, 99.0, 10));
        assert!(bid.fills.is_empty());
        assert_eq!(
            bid.outcome,
            OrderOutcome::Resting {
                remaining: Quantity(10)
            }
        );
        submit(&mut book, limit(2, Side::Sell, 101.0, 5));

        assert_eq!(book.best_bid(), Some(px(99.0)));
        assert_eq!(book.best_ask(), Some(px(101.0)));
        assert_eq!(book.bid_qty_at(px(99.0)), Quantity(10));
        assert_eq!(book.ask_qty_at(px(101.0)), Quantity(5));
        assert_eq!(book.order_count(), 2);
        assert_eq!(
            book.resting_order(OrderId(1)).map(|o| o.qty),
            Some(Quantity(10))
        );
    }

    #[test]
    fn crossing_order_trades_at_the_resting_price() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        let result = submit(&mut book, limit(2, Side::Buy, 105.0, 5));

        assert_eq!(trades(&result), vec![(2, 1, px(100.0), 5)]);
        assert_eq!(result.fills[0].taker_side, Side::Buy);
        assert_eq!(result.filled_qty, Quantity(5));
        assert_eq!(result.outcome, OrderOutcome::Filled);
        assert!(book.is_empty());
    }

    #[test]
    fn best_price_level_fills_first() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 101.0, 10));
        submit(&mut book, limit(2, Side::Sell, 100.0, 10));
        submit(&mut book, limit(3, Side::Sell, 102.0, 10));

        let result = submit(&mut book, limit(4, Side::Buy, 102.0, 25));
        assert_eq!(
            trades(&result),
            vec![
                (4, 2, px(100.0), 10),
                (4, 1, px(101.0), 10),
                (4, 3, px(102.0), 5)
            ]
        );
        assert_eq!(book.ask_levels(10), vec![(px(102.0), Quantity(5))]);
    }

    #[test]
    fn earlier_orders_fill_first_and_partial_fills_keep_priority() {
        let mut book = book();
        for id in 1..=3 {
            submit(&mut book, limit(id, Side::Sell, 100.0, 10));
        }

        let first = submit(&mut book, limit(4, Side::Buy, 100.0, 15));
        assert_eq!(
            trades(&first),
            vec![(4, 1, px(100.0), 10), (4, 2, px(100.0), 5)]
        );

        // Order 2 was partially filled but is still ahead of order 3.
        let second = submit(&mut book, limit(5, Side::Buy, 100.0, 6));
        assert_eq!(
            trades(&second),
            vec![(5, 2, px(100.0), 5), (5, 3, px(100.0), 1)]
        );
        assert_eq!(book.ask_qty_at(px(100.0)), Quantity(9));
    }

    #[test]
    fn gtc_remainder_rests_after_partial_fill() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 4));
        let result = submit(&mut book, limit(2, Side::Buy, 101.0, 10));

        assert_eq!(trades(&result), vec![(2, 1, px(100.0), 4)]);
        assert_eq!(
            result.outcome,
            OrderOutcome::Resting {
                remaining: Quantity(6)
            }
        );
        assert_eq!(book.best_bid(), Some(px(101.0)));
        assert_eq!(book.bid_qty_at(px(101.0)), Quantity(6));
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn ioc_takes_all_eligible_liquidity_then_cancels_the_rest() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Sell, 100.0, 5));
        submit(&mut book, limit(3, Side::Sell, 101.0, 5));
        submit(&mut book, limit(4, Side::Sell, 102.0, 5));

        let ioc = limit(5, Side::Buy, 101.0, 20).with_tif(TimeInForce::IOC);
        let result = submit(&mut book, ioc);

        assert_eq!(
            trades(&result),
            vec![
                (5, 1, px(100.0), 5),
                (5, 2, px(100.0), 5),
                (5, 3, px(101.0), 5)
            ]
        );
        assert_eq!(
            result.outcome,
            OrderOutcome::Cancelled {
                remaining: Quantity(5)
            }
        );
        assert_eq!(book.best_bid(), None, "IOC remainder must not rest");
        assert_eq!(book.ask_levels(10), vec![(px(102.0), Quantity(5))]);
    }

    #[test]
    fn fok_that_cannot_fill_leaves_the_book_unchanged() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Sell, 101.0, 5));
        let before = book.ask_levels(10);

        let fok = limit(3, Side::Buy, 101.0, 11).with_tif(TimeInForce::FOK);
        let result = submit(&mut book, fok);

        assert!(result.fills.is_empty());
        assert_eq!(result.filled_qty, Quantity::ZERO);
        assert_eq!(result.outcome, OrderOutcome::Killed);
        assert_eq!(book.ask_levels(10), before);
        assert_eq!(book.order_count(), 2);
    }

    #[test]
    fn fok_fills_across_levels_when_liquidity_suffices() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Sell, 101.0, 5));

        let fok = limit(3, Side::Buy, 101.0, 10).with_tif(TimeInForce::FOK);
        let result = submit(&mut book, fok);

        assert_eq!(result.outcome, OrderOutcome::Filled);
        assert_eq!(result.filled_qty, Quantity(10));
        assert!(book.is_empty());
    }

    #[test]
    fn fok_ignores_liquidity_beyond_its_limit() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Sell, 102.0, 10));

        let fok = limit(3, Side::Buy, 101.0, 10).with_tif(TimeInForce::FOK);
        let result = submit(&mut book, fok);

        assert_eq!(result.outcome, OrderOutcome::Killed);
        assert_eq!(book.ask_qty_at(px(100.0)), Quantity(5));
    }

    #[test]
    fn market_order_sweeps_and_cancels_remainder() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Sell, 150.0, 5));

        let result = submit(
            &mut book,
            Order::market(OrderId(3), Side::Buy, Quantity(20), Timestamp(3)),
        );
        assert_eq!(
            trades(&result),
            vec![(3, 1, px(100.0), 5), (3, 2, px(150.0), 5)]
        );
        assert_eq!(
            result.outcome,
            OrderOutcome::Cancelled {
                remaining: Quantity(10)
            }
        );
        assert!(book.is_empty());
    }

    #[test]
    fn market_sell_trades_against_bids_best_first() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 98.0, 5));
        submit(&mut book, limit(2, Side::Buy, 99.0, 5));

        let result = submit(
            &mut book,
            Order::market(OrderId(3), Side::Sell, Quantity(7), Timestamp(3)),
        );
        assert_eq!(
            trades(&result),
            vec![(3, 2, px(99.0), 5), (3, 1, px(98.0), 2)]
        );
        assert_eq!(result.outcome, OrderOutcome::Filled);
        assert_eq!(book.bid_levels(10), vec![(px(98.0), Quantity(3))]);
    }

    #[test]
    fn invalid_orders_are_rejected_without_side_effects() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        let before = book.ask_levels(10);

        assert_eq!(
            book.submit_order(limit(2, Side::Buy, 100.0, 0)),
            Err(OrderError::ZeroQuantity)
        );
        assert_eq!(
            book.submit_order(Order::limit(
                OrderId(3),
                Side::Buy,
                Price(0),
                Quantity(1),
                Timestamp(3)
            )),
            Err(OrderError::NonPositivePrice(0))
        );
        assert_eq!(
            book.submit_order(Order::limit(
                OrderId(4),
                Side::Buy,
                Price(-5),
                Quantity(1),
                Timestamp(4)
            )),
            Err(OrderError::NonPositivePrice(-5))
        );
        assert_eq!(
            book.submit_order(
                Order::market(OrderId(5), Side::Buy, Quantity(1), Timestamp(5))
                    .with_tif(TimeInForce::GTC)
            ),
            Err(OrderError::MarketOrderCannotRest)
        );
        assert_eq!(
            book.submit_order(limit(1, Side::Sell, 101.0, 5)),
            Err(OrderError::DuplicateOrderId(1))
        );
        assert_eq!(
            book.submit_order(limit(1, Side::Buy, 100.0, 5).with_tif(TimeInForce::IOC)),
            Err(OrderError::DuplicateOrderId(1)),
            "a duplicate id must not trade either"
        );

        assert_eq!(book.ask_levels(10), before);
        assert_eq!(book.order_count(), 1);
        book.check_invariants().unwrap();
    }

    #[test]
    fn id_of_a_filled_order_can_be_reused() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Buy, 100.0, 5));
        assert!(!book.contains_order(OrderId(1)));

        let reused = submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        assert_eq!(
            reused.outcome,
            OrderOutcome::Resting {
                remaining: Quantity(5)
            }
        );
    }

    #[test]
    fn level_quantity_overflow_is_rejected() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 100.0, u64::MAX - 1));

        assert_eq!(
            book.submit_order(limit(2, Side::Buy, 100.0, 2)),
            Err(OrderError::LevelQuantityOverflow)
        );
        submit(&mut book, limit(3, Side::Buy, 100.0, 1));
        assert_eq!(book.bid_qty_at(px(100.0)), Quantity(u64::MAX));
    }

    #[test]
    fn cancel_updates_depth_and_removes_empty_levels() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 100.0, 10));
        submit(&mut book, limit(2, Side::Buy, 100.0, 7));
        submit(&mut book, limit(3, Side::Buy, 99.0, 5));

        let cancelled = book.cancel_order(OrderId(1)).expect("order 1 rests");
        assert_eq!(cancelled.qty, Quantity(10));
        assert_eq!(book.bid_qty_at(px(100.0)), Quantity(7));
        assert_eq!(book.bid_levels(1), vec![(px(100.0), Quantity(7))]);
        book.check_invariants().unwrap();

        assert!(book.cancel_order(OrderId(2)).is_some());
        assert_eq!(book.best_bid(), Some(px(99.0)));
        assert_eq!(book.bid_qty_at(px(100.0)), Quantity::ZERO);
        book.check_invariants().unwrap();

        assert_eq!(book.cancel_order(OrderId(2)), None, "already cancelled");
        assert_eq!(book.cancel_order(OrderId(999)), None, "never existed");
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn cancel_returns_remaining_quantity_after_partial_fill() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 10));
        submit(&mut book, limit(2, Side::Buy, 100.0, 4));

        let cancelled = book.cancel_order(OrderId(1)).expect("order 1 rests");
        assert_eq!(cancelled.qty, Quantity(6));
        assert!(book.is_empty());
        book.check_invariants().unwrap();
    }

    #[test]
    fn filled_orders_cannot_be_cancelled() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 5));
        submit(&mut book, limit(2, Side::Buy, 100.0, 5));
        assert_eq!(book.cancel_order(OrderId(1)), None);
    }

    #[test]
    fn mid_and_spread() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 99.0, 10));
        submit(&mut book, limit(2, Side::Sell, 101.0, 10));
        assert_eq!(book.mid(), Some(px(100.0)));
        assert_eq!(book.spread(), Some(px(2.0)));

        let mut odd = OrderBook::new("ODD".to_string());
        submit(
            &mut odd,
            Order::limit(OrderId(1), Side::Buy, Price(100), Quantity(1), Timestamp(1)),
        );
        submit(
            &mut odd,
            Order::limit(
                OrderId(2),
                Side::Sell,
                Price(103),
                Quantity(1),
                Timestamp(2),
            ),
        );
        assert_eq!(odd.mid(), Some(Price(101)), "mid rounds down to a tick");
        assert_eq!(odd.spread(), Some(Price(3)));
    }

    #[test]
    fn depth_snapshots_are_best_first_and_aggregated() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 99.0, 10));
        submit(&mut book, limit(2, Side::Buy, 98.0, 20));
        submit(&mut book, limit(3, Side::Buy, 99.0, 1));
        submit(&mut book, limit(4, Side::Sell, 101.0, 15));
        submit(&mut book, limit(5, Side::Sell, 102.0, 25));

        assert_eq!(
            book.bid_levels(5),
            vec![(px(99.0), Quantity(11)), (px(98.0), Quantity(20))]
        );
        assert_eq!(book.ask_levels(1), vec![(px(101.0), Quantity(15))]);
    }

    #[test]
    fn top_of_book_is_the_best_of_several_levels() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 98.0, 10));
        submit(&mut book, limit(2, Side::Buy, 99.0, 10));
        submit(&mut book, limit(3, Side::Sell, 102.0, 10));
        submit(&mut book, limit(4, Side::Sell, 101.0, 10));

        assert_eq!(book.best_bid(), Some(px(99.0)), "highest bid wins");
        assert_eq!(book.best_ask(), Some(px(101.0)), "lowest ask wins");
        assert_eq!(book.mid(), Some(px(100.0)));
        assert_eq!(book.spread(), Some(px(2.0)));
    }

    #[test]
    fn mid_does_not_overflow_at_extreme_prices() {
        let mut book = book();
        let extreme = |id: u64, side: Side, ticks: i64| {
            Order::limit(OrderId(id), side, Price(ticks), Quantity(1), Timestamp(id))
        };
        submit(&mut book, extreme(1, Side::Buy, i64::MAX - 1));
        submit(&mut book, extreme(2, Side::Sell, i64::MAX));

        // Adding the two prices overflows i64, so the sum is taken in i128.
        assert_eq!(book.mid(), Some(Price(i64::MAX - 1)));
        assert_eq!(book.spread(), Some(Price(1)));
    }

    #[test]
    fn resting_order_returns_the_order_with_that_id() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 10));
        submit(&mut book, limit(2, Side::Sell, 100.0, 7));

        // Both orders share a level, so each id must return its own order and
        // its own quantity, not the front of the queue.
        let resting = |book: &OrderBook, id: u64| {
            book.resting_order(OrderId(id))
                .map(|order| (order.id, order.qty))
        };
        assert_eq!(resting(&book, 1), Some((OrderId(1), Quantity(10))));
        assert_eq!(resting(&book, 2), Some((OrderId(2), Quantity(7))));

        // A partial fill of the front order shows in its remaining quantity,
        // and leaves the order behind it untouched.
        submit(&mut book, limit(3, Side::Buy, 100.0, 4));
        assert_eq!(resting(&book, 1), Some((OrderId(1), Quantity(6))));
        assert_eq!(resting(&book, 2), Some((OrderId(2), Quantity(7))));
        assert_eq!(resting(&book, 99), None);
    }

    #[test]
    fn contains_order_tracks_resting_orders_on_both_sides() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Buy, 99.0, 10));
        submit(&mut book, limit(2, Side::Sell, 101.0, 10));
        assert!(book.contains_order(OrderId(1)), "resting bid");
        assert!(book.contains_order(OrderId(2)), "resting ask");
        assert!(!book.contains_order(OrderId(3)), "never submitted");

        book.cancel_order(OrderId(1)).expect("order 1 rests");
        assert!(!book.contains_order(OrderId(1)), "cancelled");

        let taker = submit(&mut book, limit(4, Side::Buy, 101.0, 10));
        assert_eq!(taker.outcome, OrderOutcome::Filled);
        assert!(!book.contains_order(OrderId(2)), "fully filled maker");
        assert!(!book.contains_order(OrderId(4)), "fully filled taker");
        assert!(book.is_empty());
    }

    #[test]
    fn fok_fills_a_quantity_that_saturates_the_depth_sum() {
        let mut book = book();
        submit(&mut book, limit(1, Side::Sell, 100.0, 10));
        submit(
            &mut book,
            Order::limit(
                OrderId(2),
                Side::Sell,
                px(101.0),
                Quantity(u64::MAX),
                Timestamp(2),
            ),
        );

        // Summing the two levels overflows u64, so the available quantity is
        // saturated and capped at the order size instead of wrapping.
        let fok = Order::limit(
            OrderId(3),
            Side::Buy,
            px(101.0),
            Quantity(u64::MAX),
            Timestamp(3),
        )
        .with_tif(TimeInForce::FOK);
        let result = submit(&mut book, fok);

        assert_eq!(result.outcome, OrderOutcome::Filled);
        assert_eq!(result.filled_qty, Quantity(u64::MAX));
        // It took all 10 at 100.0 and u64::MAX - 10 at 101.0.
        assert_eq!(book.ask_levels(10), vec![(px(101.0), Quantity(10))]);
    }

    #[test]
    fn check_invariants_detects_corrupted_state() {
        // Each case corrupts the book through private state, which submit_order
        // and cancel_order cannot produce, and asserts the violation is caught.
        let violation = |book: &OrderBook| book.check_invariants().expect_err("must be rejected");

        let mut crossed = book();
        submit(&mut crossed, limit(1, Side::Sell, 100.0, 5));
        crossed.rest(limit(2, Side::Buy, 101.0, 5), px(101.0));
        assert!(violation(&crossed).contains("crossed"));

        let mut stale_total = book();
        submit(&mut stale_total, limit(1, Side::Buy, 99.0, 5));
        stale_total.bids.get_mut(&px(99.0)).unwrap().total_qty += 1;
        assert!(violation(&stale_total).contains("caches"));

        let mut missing_index = book();
        submit(&mut missing_index, limit(1, Side::Buy, 99.0, 5));
        missing_index.index.remove(&OrderId(1));
        assert!(violation(&missing_index).contains("missing or stale"));

        let mut stale_index = book();
        submit(&mut stale_index, limit(1, Side::Buy, 99.0, 5));
        stale_index.index.insert(OrderId(1), (Side::Buy, px(98.0)));
        assert!(violation(&stale_index).contains("missing or stale"));

        // The same id resting at two levels. The index can only point at one of
        // them, so the order count still matches and only the id check catches it.
        let mut duplicate = book();
        submit(&mut duplicate, limit(1, Side::Buy, 99.0, 5));
        duplicate.rest(limit(1, Side::Buy, 98.0, 5), px(98.0));
        assert_eq!(duplicate.order_count(), 1);
        assert!(violation(&duplicate).contains("rests twice"));

        let mut empty_level = book();
        submit(&mut empty_level, limit(1, Side::Buy, 99.0, 5));
        empty_level.bids.insert(px(97.0), Level::default());
        assert!(violation(&empty_level).contains("empty"));

        let mut resting_ioc = book();
        resting_ioc.rest(
            limit(1, Side::Buy, 99.0, 5).with_tif(TimeInForce::IOC),
            px(99.0),
        );
        assert!(violation(&resting_ioc).contains("IOC"));
    }
}
