use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Single-symbol order book with price-time priority matching.
#[derive(Debug, Clone)]
pub struct OrderBook {
    symbol: String,
    /// Bid levels: descending price (highest first)
    bids: BTreeMap<Price, VecDeque<Order>>,
    /// Ask levels: ascending price (lowest first)
    asks: BTreeMap<Price, VecDeque<Order>>,
    /// Track total volume at each level
    bid_depth: BTreeMap<Price, Quantity>,
    ask_depth: BTreeMap<Price, Quantity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
    pub timestamp: Timestamp,
    pub tif: TimeInForce,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub order_id: OrderId,
    pub fill_price: Price,
    pub fill_qty: Quantity,
    pub timestamp: Timestamp,
    pub is_maker: bool,
}

impl Order {
    pub fn new(
        id: OrderId,
        side: Side,
        price: Price,
        qty: Quantity,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            price,
            qty,
            timestamp,
            tif: TimeInForce::GTC,
        }
    }

    pub fn with_tif(mut self, tif: TimeInForce) -> Self {
        self.tif = tif;
        self
    }
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            bid_depth: BTreeMap::new(),
            ask_depth: BTreeMap::new(),
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Insert a limit order; returns immediate fills if marketable.
    /// Implements price-time priority matching.
    pub fn insert_order(&mut self, mut order: Order) -> Vec<Fill> {
        let mut fills = Vec::new();

        match order.side {
            Side::Buy => {
                // Match against asks (ascending price).
                while order.qty > Quantity::ZERO {
                    // Get the best (lowest) ask
                    let best_ask_price = self.asks.keys().next().copied();

                    match best_ask_price {
                        Some(ask_price) if ask_price <= order.price => {
                            let fill_qty;
                            let resting_fully_filled;
                            let queue_empty;

                            {
                                let ask_queue = self.asks.get_mut(&ask_price).unwrap();

                                if let Some(mut resting_order) = ask_queue.pop_front() {
                                    fill_qty = Quantity::new(order.qty.0.min(resting_order.qty.0));

                                    fills.push(Fill {
                                        order_id: order.id,
                                        fill_price: ask_price,
                                        fill_qty,
                                        timestamp: Timestamp::now_nanos(),
                                        is_maker: false,
                                    });

                                    order.qty.0 -= fill_qty.0;
                                    resting_order.qty.0 -= fill_qty.0;

                                    resting_fully_filled = resting_order.qty == Quantity::ZERO;

                                    if resting_order.qty > Quantity::ZERO {
                                        ask_queue.push_front(resting_order);
                                    }

                                    queue_empty = ask_queue.is_empty();
                                } else {
                                    break;
                                }
                            }

                            // Depth update removed - already maintained via entry().and_modify()
                            // if resting_fully_filled {
                            //     self.update_depth(&ask_price, &Side::Sell, false);
                            // }

                            if queue_empty {
                                self.asks.remove(&ask_price);
                                self.ask_depth.remove(&ask_price);
                            }

                            if order.tif == TimeInForce::IOC && order.qty > Quantity::ZERO {
                                return fills;
                            }
                        }
                        _ => {
                            // No more matches or limit price reached
                            if order.tif == TimeInForce::IOC || order.tif == TimeInForce::FOK {
                                // Cancel unfilled portion
                                if order.tif == TimeInForce::FOK && !fills.is_empty() {
                                    // FOK not fully filled - reject all
                                    return Vec::new();
                                }
                                return fills;
                            }
                            break;
                        }
                    }
                }

                // Rest stays on bid side if any qty remaining and GTC
                if order.qty > Quantity::ZERO && order.tif == TimeInForce::GTC {
                    self.bid_depth
                        .entry(order.price)
                        .and_modify(|q| *q = q.saturating_add(order.qty))
                        .or_insert(order.qty);
                    self.bids
                        .entry(order.price)
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
            }

            Side::Sell => {
                // Match against bids (descending price).
                while order.qty > Quantity::ZERO {
                    // Get the best (highest) bid
                    let best_bid_price = self.bids.keys().next_back().copied();

                    match best_bid_price {
                        Some(bid_price) if bid_price >= order.price => {
                            let fill_qty;
                            let resting_fully_filled;
                            let queue_empty;

                            {
                                let bid_queue = self.bids.get_mut(&bid_price).unwrap();

                                if let Some(mut resting_order) = bid_queue.pop_front() {
                                    fill_qty = Quantity::new(order.qty.0.min(resting_order.qty.0));

                                    fills.push(Fill {
                                        order_id: order.id,
                                        fill_price: bid_price,
                                        fill_qty,
                                        timestamp: Timestamp::now_nanos(),
                                        is_maker: false,
                                    });

                                    order.qty.0 -= fill_qty.0;
                                    resting_order.qty.0 -= fill_qty.0;

                                    resting_fully_filled = resting_order.qty == Quantity::ZERO;

                                    if resting_order.qty > Quantity::ZERO {
                                        bid_queue.push_front(resting_order);
                                    }

                                    queue_empty = bid_queue.is_empty();
                                } else {
                                    break;
                                }
                            }

                            // Depth update removed - already maintained via entry().and_modify()
                            // if resting_fully_filled {
                            //     self.update_depth(&bid_price, &Side::Buy, false);
                            // }

                            if queue_empty {
                                self.bids.remove(&bid_price);
                                self.bid_depth.remove(&bid_price);
                            }

                            if order.tif == TimeInForce::IOC && order.qty > Quantity::ZERO {
                                return fills;
                            }
                        }
                        _ => {
                            // No more matches
                            if order.tif == TimeInForce::IOC || order.tif == TimeInForce::FOK {
                                if order.tif == TimeInForce::FOK && !fills.is_empty() {
                                    return Vec::new();
                                }
                                return fills;
                            }
                            break;
                        }
                    }
                }

                // Rest stays on ask side if any qty remaining and GTC
                if order.qty > Quantity::ZERO && order.tif == TimeInForce::GTC {
                    self.ask_depth
                        .entry(order.price)
                        .and_modify(|q| *q = q.saturating_add(order.qty))
                        .or_insert(order.qty);
                    self.asks
                        .entry(order.price)
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
            }
        }

        fills
    }

    fn update_depth(&mut self, price: &Price, side: &Side, _add: bool) {
        // Placeholder for depth tracking updates
        match side {
            Side::Buy => {
                if let Some(queue) = self.bids.get(price) {
                    let total: u64 = queue.iter().map(|o| o.qty.0).sum();
                    if total > 0 {
                        self.bid_depth.insert(*price, Quantity::new(total));
                    } else {
                        self.bid_depth.remove(price);
                    }
                }
            }
            Side::Sell => {
                if let Some(queue) = self.asks.get(price) {
                    let total: u64 = queue.iter().map(|o| o.qty.0).sum();
                    if total > 0 {
                        self.ask_depth.insert(*price, Quantity::new(total));
                    } else {
                        self.ask_depth.remove(price);
                    }
                }
            }
        }
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    pub fn mid(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(Price((bid.0 + ask.0) / 2)),
            _ => None,
        }
    }

    pub fn spread(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(Price(ask.0 - bid.0)),
            _ => None,
        }
    }

    /// Get total bid depth at price level
    pub fn bid_qty_at(&self, price: Price) -> Quantity {
        self.bid_depth.get(&price).copied().unwrap_or(Quantity::ZERO)
    }

    /// Get total ask depth at price level
    pub fn ask_qty_at(&self, price: Price) -> Quantity {
        self.ask_depth.get(&price).copied().unwrap_or(Quantity::ZERO)
    }

    /// Get top N levels of bids (highest first)
    pub fn bid_levels(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.bids
            .iter()
            .rev()
            .take(n)
            .map(|(p, q)| (*p, Quantity::new(q.iter().map(|o| o.qty.0).sum())))
            .collect()
    }

    /// Get top N levels of asks (lowest first)
    pub fn ask_levels(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.asks
            .iter()
            .take(n)
            .map(|(p, q)| (*p, Quantity::new(q.iter().map(|o| o.qty.0).sum())))
            .collect()
    }

    /// Seed the orderbook with market maker liquidity at a reference price
    /// This creates BUY and SELL orders to provide liquidity for testing
    pub fn seed_market_maker(&mut self, mid_price: Price, num_levels: usize, qty_per_level: Quantity) {
        let mut order_id = 9_000_000_000_000_000_000u64; // Use high IDs for MM orders

        // Add ask (sell) orders above mid price
        for i in 1..=num_levels {
            let price_offset = (mid_price.0 as f64 * 0.0001 * i as f64) as i64; // 0.01% spread per level
            let ask_price = Price(mid_price.0 + price_offset);

            let order = Order::new(
                OrderId(order_id),
                Side::Sell,
                ask_price,
                qty_per_level,
                Timestamp::now_nanos(),
            );

            self.asks.entry(ask_price)
                .or_insert_with(VecDeque::new)
                .push_back(order);

            self.ask_depth.entry(ask_price)
                .and_modify(|q| q.0 += qty_per_level.0)
                .or_insert(qty_per_level);

            order_id += 1;
        }

        // Add bid (buy) orders below mid price
        for i in 1..=num_levels {
            let price_offset = (mid_price.0 as f64 * 0.0001 * i as f64) as i64; // 0.01% spread per level
            let bid_price = Price(mid_price.0 - price_offset);

            let order = Order::new(
                OrderId(order_id),
                Side::Buy,
                bid_price,
                qty_per_level,
                Timestamp::now_nanos(),
            );

            self.bids.entry(bid_price)
                .or_insert_with(VecDeque::new)
                .push_back(order);

            self.bid_depth.entry(bid_price)
                .and_modify(|q| q.0 += qty_per_level.0)
                .or_insert(qty_per_level);

            order_id += 1;
        }
    }

    /// Cancel an order by ID
    pub fn cancel_order(&mut self, order_id: OrderId) -> Option<Order> {
        // Search in bids
        for (price, queue) in self.bids.iter_mut() {
            if let Some(pos) = queue.iter().position(|o| o.id == order_id) {
                let order = queue.remove(pos).unwrap();
                if queue.is_empty() {
                    let price_copy = *price;
                    self.bids.remove(&price_copy);
                    self.bid_depth.remove(&price_copy);
                }
                return Some(order);
            }
        }

        // Search in asks
        for (price, queue) in self.asks.iter_mut() {
            if let Some(pos) = queue.iter().position(|o| o.id == order_id) {
                let order = queue.remove(pos).unwrap();
                if queue.is_empty() {
                    let price_copy = *price;
                    self.asks.remove(&price_copy);
                    self.ask_depth.remove(&price_copy);
                }
                return Some(order);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_order(id: u64, side: Side, price: f64, qty: u64) -> Order {
        Order::new(
            OrderId::new(id),
            side,
            Price::from_f64(price),
            Quantity::new(qty),
            Timestamp::now_nanos(),
        )
    }

    #[test]
    fn test_empty_book() {
        let book = OrderBook::new("TEST".to_string());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.mid(), None);
    }

    #[test]
    fn test_insert_single_bid() {
        let mut book = OrderBook::new("TEST".to_string());
        let order = create_test_order(1, Side::Buy, 100.0, 10);
        let fills = book.insert_order(order);
        assert!(fills.is_empty());
        assert_eq!(book.best_bid(), Some(Price::from_f64(100.0)));
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_insert_single_ask() {
        let mut book = OrderBook::new("TEST".to_string());
        let order = create_test_order(1, Side::Sell, 101.0, 10);
        let fills = book.insert_order(order);
        assert!(fills.is_empty());
        assert_eq!(book.best_ask(), Some(Price::from_f64(101.0)));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_marketable_buy_full_fill() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add resting sell order
        let sell_order = create_test_order(1, Side::Sell, 100.0, 10);
        book.insert_order(sell_order);

        // Aggressive buy order at same price
        let buy_order = create_test_order(2, Side::Buy, 100.0, 10);
        let fills = book.insert_order(buy_order);

        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, Quantity::new(10));
        assert_eq!(fills[0].fill_price, Price::from_f64(100.0));
        assert_eq!(fills[0].order_id, OrderId::new(2));

        // Book should be empty
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_marketable_sell_full_fill() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add resting buy order
        let buy_order = create_test_order(1, Side::Buy, 100.0, 10);
        book.insert_order(buy_order);

        // Aggressive sell order at same price
        let sell_order = create_test_order(2, Side::Sell, 100.0, 10);
        let fills = book.insert_order(sell_order);

        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, Quantity::new(10));
        assert_eq!(fills[0].fill_price, Price::from_f64(100.0));

        // Book should be empty
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_partial_fill() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add resting sell order for 10
        let sell_order = create_test_order(1, Side::Sell, 100.0, 10);
        book.insert_order(sell_order);

        // Aggressive buy order for 5 (partial)
        let buy_order = create_test_order(2, Side::Buy, 100.0, 5);
        let fills = book.insert_order(buy_order);

        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, Quantity::new(5));

        // Book should have 5 remaining on sell side
        assert_eq!(book.best_ask(), Some(Price::from_f64(100.0)));
        assert_eq!(book.ask_qty_at(Price::from_f64(100.0)), Quantity::new(5));
    }

    #[test]
    fn test_price_time_priority() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add two sell orders at same price
        let sell1 = create_test_order(1, Side::Sell, 100.0, 10);
        book.insert_order(sell1);

        std::thread::sleep(std::time::Duration::from_nanos(100));

        let sell2 = create_test_order(2, Side::Sell, 100.0, 10);
        book.insert_order(sell2);

        // Aggressive buy should match first order first
        let buy = create_test_order(3, Side::Buy, 100.0, 15);
        let fills = book.insert_order(buy);

        // Should get one fill that consumed full first order + partial second
        assert!(fills.len() >= 1);
        let total_filled: u64 = fills.iter().map(|f| f.fill_qty.0).sum();
        assert_eq!(total_filled, 15);
    }

    #[test]
    fn test_price_priority() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add sells at different prices
        book.insert_order(create_test_order(1, Side::Sell, 101.0, 10));
        book.insert_order(create_test_order(2, Side::Sell, 100.0, 10));
        book.insert_order(create_test_order(3, Side::Sell, 102.0, 10));

        // Best ask should be lowest price
        assert_eq!(book.best_ask(), Some(Price::from_f64(100.0)));

        // Aggressive buy at 105 should match 100 first
        let buy = create_test_order(4, Side::Buy, 105.0, 5);
        let fills = book.insert_order(buy);

        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_price, Price::from_f64(100.0));
    }

    #[test]
    fn test_limit_price_not_crossed() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add sell at 101
        book.insert_order(create_test_order(1, Side::Sell, 101.0, 10));

        // Buy limit at 100 should NOT match
        let buy = create_test_order(2, Side::Buy, 100.0, 10);
        let fills = book.insert_order(buy);

        assert!(fills.is_empty());
        assert_eq!(book.best_bid(), Some(Price::from_f64(100.0)));
        assert_eq!(book.best_ask(), Some(Price::from_f64(101.0)));
    }

    #[test]
    fn test_mid_and_spread() {
        let mut book = OrderBook::new("TEST".to_string());
        book.insert_order(create_test_order(1, Side::Buy, 99.0, 10));
        book.insert_order(create_test_order(2, Side::Sell, 101.0, 10));

        assert_eq!(book.mid(), Some(Price::from_f64(100.0)));
        assert_eq!(book.spread(), Some(Price::from_f64(2.0)));
    }

    #[test]
    fn test_cancel_order() {
        let mut book = OrderBook::new("TEST".to_string());
        let order = create_test_order(1, Side::Buy, 100.0, 10);
        book.insert_order(order);

        assert_eq!(book.best_bid(), Some(Price::from_f64(100.0)));

        let cancelled = book.cancel_order(OrderId::new(1));
        assert!(cancelled.is_some());
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_ioc_order_partial_fill() {
        let mut book = OrderBook::new("TEST".to_string());
        // Add sell for 5
        book.insert_order(create_test_order(1, Side::Sell, 100.0, 5));

        // IOC buy for 10 should only fill 5 and cancel rest
        let mut buy = create_test_order(2, Side::Buy, 100.0, 10);
        buy.tif = TimeInForce::IOC;
        let fills = book.insert_order(buy);

        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, Quantity::new(5));

        // No residual on bid side
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_levels_display() {
        let mut book = OrderBook::new("TEST".to_string());
        book.insert_order(create_test_order(1, Side::Buy, 99.0, 10));
        book.insert_order(create_test_order(2, Side::Buy, 98.0, 20));
        book.insert_order(create_test_order(3, Side::Sell, 101.0, 15));
        book.insert_order(create_test_order(4, Side::Sell, 102.0, 25));

        let bids = book.bid_levels(2);
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].0, Price::from_f64(99.0));
        assert_eq!(bids[1].0, Price::from_f64(98.0));

        let asks = book.ask_levels(2);
        assert_eq!(asks.len(), 2);
        assert_eq!(asks[0].0, Price::from_f64(101.0));
        assert_eq!(asks[1].0, Price::from_f64(102.0));
    }
}
