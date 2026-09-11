use api::{ParentOrder, Position};
use orderbook::{Fill, Order, OrderId, Quantity, Side};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a child order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildOrderInfo {
    pub id: OrderId,
    pub parent_id: u64,
    pub status: String,
    pub filled_qty: Quantity,
    pub target_time_ns: u64,
}

/// Execution engine state.
#[derive(Debug)]
pub struct EngineState {
    /// Parent orders by ID
    pub parent_orders: HashMap<u64, ParentOrder>,

    /// Child order tracking
    pub child_orders: HashMap<OrderId, ChildOrderInfo>,

    /// Position tracking by symbol
    pub positions: HashMap<String, Position>,

    /// All fills
    pub fills: Vec<Fill>,

    /// Pending child orders (target_time_ns, order)
    pub pending_children: Vec<(u64, Order)>,

    /// Next parent order ID
    next_parent_id: u64,

    /// Next child order ID
    next_child_id: u64,
}

impl EngineState {
    pub fn new() -> Self {
        Self {
            parent_orders: HashMap::new(),
            child_orders: HashMap::new(),
            positions: HashMap::new(),
            fills: Vec::new(),
            pending_children: Vec::new(),
            next_parent_id: 1,
            next_child_id: 1000,
        }
    }

    pub fn next_parent_id(&mut self) -> u64 {
        let id = self.next_parent_id;
        self.next_parent_id += 1;
        id
    }

    pub fn next_child_id(&mut self) -> OrderId {
        let id = OrderId::new(self.next_child_id);
        self.next_child_id += 1;
        id
    }

    pub fn add_parent_order(&mut self, order: ParentOrder) {
        self.parent_orders.insert(order.id, order);
    }

    pub fn get_parent_order(&self, id: u64) -> Option<&ParentOrder> {
        self.parent_orders.get(&id)
    }

    pub fn get_parent_order_mut(&mut self, id: u64) -> Option<&mut ParentOrder> {
        self.parent_orders.get_mut(&id)
    }

    pub fn add_child_order(&mut self, info: ChildOrderInfo) {
        self.child_orders.insert(info.id, info);
    }

    pub fn get_child_order(&self, id: &OrderId) -> Option<&ChildOrderInfo> {
        self.child_orders.get(id)
    }

    pub fn get_child_order_mut(&mut self, id: &OrderId) -> Option<&mut ChildOrderInfo> {
        self.child_orders.get_mut(id)
    }

    /// Update position based on a fill.
    pub fn update_position(&mut self, symbol: &str, fill: &Fill, side: Side) {
        let position = self
            .positions
            .entry(symbol.to_string())
            .or_insert_with(|| Position {
                symbol: symbol.to_string(),
                quantity: 0,
                avg_price: 0.0,
                realized_pnl: 0.0,
                unrealized_pnl: 0.0,
            });

        let fill_qty = fill.qty.value() as i64;
        let fill_price = fill.price.as_f64();

        match side {
            Side::Buy => {
                // Update average price
                if position.quantity >= 0 {
                    // Increasing long position
                    let total_cost = position.avg_price * position.quantity as f64
                        + fill_price * fill_qty as f64;

                    // Use checked arithmetic to prevent overflow
                    let new_qty = position
                        .quantity
                        .checked_add(fill_qty)
                        .expect("Position quantity overflow on buy - quantity too large");

                    position.quantity = new_qty;
                    position.avg_price = total_cost / position.quantity as f64;
                } else {
                    // Reducing short position
                    let pnl = (position.avg_price - fill_price) * fill_qty as f64;
                    position.realized_pnl += pnl;

                    let new_qty = position
                        .quantity
                        .checked_add(fill_qty)
                        .expect("Position quantity overflow while reducing short");

                    position.quantity = new_qty;
                    if position.quantity == 0 {
                        position.avg_price = 0.0;
                    }
                }
            }
            Side::Sell => {
                if position.quantity <= 0 {
                    // Increasing short position
                    let total_cost = position.avg_price * (-position.quantity) as f64
                        + fill_price * fill_qty as f64;

                    let new_qty = position
                        .quantity
                        .checked_sub(fill_qty)
                        .expect("Position quantity underflow on sell - quantity too large");

                    position.quantity = new_qty;
                    position.avg_price = total_cost / (-position.quantity) as f64;
                } else {
                    // Reducing long position
                    let pnl = (fill_price - position.avg_price) * fill_qty as f64;
                    position.realized_pnl += pnl;

                    let new_qty = position
                        .quantity
                        .checked_sub(fill_qty)
                        .expect("Position quantity underflow while reducing long");

                    position.quantity = new_qty;
                    if position.quantity == 0 {
                        position.avg_price = 0.0;
                    }
                }
            }
        }
    }

    pub fn get_position(&self, symbol: &str) -> Option<&Position> {
        self.positions.get(symbol)
    }

    /// Get position with calculated unrealized PnL based on current market price
    pub fn get_position_with_pnl(&self, symbol: &str, current_price: f64) -> Option<Position> {
        self.positions.get(symbol).map(|pos| {
            let mut position = pos.clone();
            // Calculate unrealized PnL
            // For long positions: (current_price - avg_price) * quantity
            // For short positions: (avg_price - current_price) * abs(quantity)
            position.unrealized_pnl =
                (current_price - position.avg_price) * position.quantity as f64;
            position
        })
    }

    pub fn add_fill(&mut self, fill: Fill) {
        self.fills.push(fill);
    }

    /// Get child orders for a parent
    pub fn get_children_for_parent(&self, parent_id: u64) -> Vec<&ChildOrderInfo> {
        self.child_orders
            .values()
            .filter(|c| c.parent_id == parent_id)
            .collect()
    }
}

impl Default for EngineState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orderbook::{Price, Timestamp};

    fn fill(taker: u64, side: Side, price: f64, qty: u64) -> Fill {
        Fill {
            taker_order_id: OrderId::new(taker),
            maker_order_id: OrderId::new(0),
            taker_side: side,
            price: Price::from_f64(price),
            qty: Quantity::new(qty),
            timestamp: Timestamp::new(0),
        }
    }

    #[test]
    fn test_position_long_build() {
        let mut state = EngineState::new();

        state.update_position("BTC", &fill(1, Side::Buy, 100.0, 10), Side::Buy);
        let pos = state.get_position("BTC").unwrap();
        assert_eq!(pos.quantity, 10);
        assert!((pos.avg_price - 100.0).abs() < 0.01);

        // Add more at different price
        state.update_position("BTC", &fill(2, Side::Buy, 105.0, 10), Side::Buy);
        let pos = state.get_position("BTC").unwrap();
        assert_eq!(pos.quantity, 20);
        assert!((pos.avg_price - 102.5).abs() < 0.01); // (100*10 + 105*10) / 20
    }

    #[test]
    fn test_position_round_trip() {
        let mut state = EngineState::new();

        // Buy 10 at 100
        state.update_position("BTC", &fill(1, Side::Buy, 100.0, 10), Side::Buy);

        // Sell 10 at 110
        state.update_position("BTC", &fill(2, Side::Sell, 110.0, 10), Side::Sell);

        let pos = state.get_position("BTC").unwrap();
        assert_eq!(pos.quantity, 0);
        assert!((pos.realized_pnl - 100.0).abs() < 0.01); // 10 * (110 - 100)
    }

    #[test]
    fn test_next_id_generation() {
        let mut state = EngineState::new();
        assert_eq!(state.next_parent_id(), 1);
        assert_eq!(state.next_parent_id(), 2);
        assert_eq!(state.next_child_id(), OrderId::new(1000));
        assert_eq!(state.next_child_id(), OrderId::new(1001));
    }
}
