use crate::types::*;
use serde::{Deserialize, Serialize};

/// Advanced order type enumeration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Standard limit order
    Limit,
    /// Market order (immediate execution at best available price)
    Market,
    /// Stop-Loss: triggers a market/limit order when price reaches stop price
    StopLoss(StopLossParams),
    /// Take-Profit: triggers a limit order when price reaches target
    TakeProfit(TakeProfitParams),
    /// Trailing Stop: dynamically adjusts stop price based on market movement
    TrailingStop(TrailingStopParams),
    /// Iceberg: only shows a portion of the total order quantity
    Iceberg(IcebergParams),
    /// Post-Only: only adds liquidity, cancels if would take liquidity
    PostOnly,
}

/// Stop-Loss order parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopLossParams {
    /// Price at which the order triggers
    pub stop_price: Price,
    /// Whether to use limit or market order when triggered
    pub use_limit: bool,
    /// Limit price if use_limit is true (optional)
    pub limit_price: Option<Price>,
}

/// Take-Profit order parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeProfitParams {
    /// Target price at which to take profit
    pub target_price: Price,
    /// Limit price for the order (typically same as target)
    pub limit_price: Price,
}

/// Trailing Stop order parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailingStopParams {
    /// Trail amount (absolute price distance)
    pub trail_amount: Price,
    /// Trail percentage (alternative to trail_amount, 0 if using trail_amount)
    pub trail_pct: u64, // Basis points (10000 = 100%)
    /// Highest/lowest price seen (updated dynamically)
    pub extreme_price: Price,
    /// Current stop price
    pub stop_price: Price,
}

/// Iceberg order parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IcebergParams {
    /// Total order quantity
    pub total_qty: Quantity,
    /// Visible quantity (refreshed as it fills)
    pub visible_qty: Quantity,
    /// Quantity already filled
    pub filled_qty: Quantity,
}

/// Advanced order wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedOrder {
    pub id: OrderId,
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
    pub timestamp: Timestamp,
    pub tif: TimeInForce,
    pub order_type: OrderType,
    /// Whether the order has been triggered (for conditional orders)
    pub triggered: bool,
}

impl AdvancedOrder {
    pub fn new_limit(
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
            order_type: OrderType::Limit,
            triggered: true, // Limit orders are immediately active
        }
    }

    pub fn new_market(
        id: OrderId,
        side: Side,
        qty: Quantity,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            price: Price::ZERO, // Market orders don't have a price
            qty,
            timestamp,
            tif: TimeInForce::IOC, // Market orders are typically IOC
            order_type: OrderType::Market,
            triggered: true,
        }
    }

    pub fn new_stop_loss(
        id: OrderId,
        side: Side,
        qty: Quantity,
        stop_price: Price,
        use_limit: bool,
        limit_price: Option<Price>,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            price: limit_price.unwrap_or(stop_price),
            qty,
            timestamp,
            tif: TimeInForce::GTC,
            order_type: OrderType::StopLoss(StopLossParams {
                stop_price,
                use_limit,
                limit_price,
            }),
            triggered: false,
        }
    }

    pub fn new_take_profit(
        id: OrderId,
        side: Side,
        qty: Quantity,
        target_price: Price,
        limit_price: Price,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            price: limit_price,
            qty,
            timestamp,
            tif: TimeInForce::GTC,
            order_type: OrderType::TakeProfit(TakeProfitParams {
                target_price,
                limit_price,
            }),
            triggered: false,
        }
    }

    pub fn new_trailing_stop(
        id: OrderId,
        side: Side,
        qty: Quantity,
        trail_amount: Price,
        trail_pct: u64,
        initial_price: Price,
        timestamp: Timestamp,
    ) -> Self {
        let stop_price = match side {
            Side::Buy => Price::new(initial_price.ticks() + trail_amount.ticks()),
            Side::Sell => Price::new(initial_price.ticks() - trail_amount.ticks()),
        };

        Self {
            id,
            side,
            price: stop_price,
            qty,
            timestamp,
            tif: TimeInForce::GTC,
            order_type: OrderType::TrailingStop(TrailingStopParams {
                trail_amount,
                trail_pct,
                extreme_price: initial_price,
                stop_price,
            }),
            triggered: false,
        }
    }

    pub fn new_iceberg(
        id: OrderId,
        side: Side,
        price: Price,
        total_qty: Quantity,
        visible_qty: Quantity,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            id,
            side,
            price,
            qty: visible_qty, // Start with visible qty
            timestamp,
            tif: TimeInForce::GTC,
            order_type: OrderType::Iceberg(IcebergParams {
                total_qty,
                visible_qty,
                filled_qty: Quantity::ZERO,
            }),
            triggered: true, // Iceberg orders are immediately active
        }
    }

    pub fn new_post_only(
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
            order_type: OrderType::PostOnly,
            triggered: true,
        }
    }

    /// Check if stop-loss should trigger based on current market price
    pub fn check_stop_loss_trigger(&self, market_price: Price) -> bool {
        if let OrderType::StopLoss(params) = &self.order_type {
            match self.side {
                // Buy stop: triggers when market rises above stop price
                Side::Buy => market_price >= params.stop_price,
                // Sell stop: triggers when market falls below stop price
                Side::Sell => market_price <= params.stop_price,
            }
        } else {
            false
        }
    }

    /// Check if take-profit should trigger
    pub fn check_take_profit_trigger(&self, market_price: Price) -> bool {
        if let OrderType::TakeProfit(params) = &self.order_type {
            match self.side {
                // Buy take-profit: triggers when market falls to target (buy low)
                Side::Buy => market_price <= params.target_price,
                // Sell take-profit: triggers when market rises to target (sell high)
                Side::Sell => market_price >= params.target_price,
            }
        } else {
            false
        }
    }

    /// Update trailing stop based on market price
    pub fn update_trailing_stop(&mut self, market_price: Price) -> bool {
        if let OrderType::TrailingStop(params) = &mut self.order_type {
            let mut updated = false;

            match self.side {
                Side::Sell => {
                    // For sell orders, track highest price
                    if market_price > params.extreme_price {
                        params.extreme_price = market_price;
                        params.stop_price = Price::new(
                            market_price.ticks() - params.trail_amount.ticks()
                        );
                        self.price = params.stop_price;
                        updated = true;
                    }

                    // Check if stop should trigger
                    if market_price <= params.stop_price {
                        self.triggered = true;
                        return true;
                    }
                }
                Side::Buy => {
                    // For buy orders, track lowest price
                    if market_price < params.extreme_price {
                        params.extreme_price = market_price;
                        params.stop_price = Price::new(
                            market_price.ticks() + params.trail_amount.ticks()
                        );
                        self.price = params.stop_price;
                        updated = true;
                    }

                    // Check if stop should trigger
                    if market_price >= params.stop_price {
                        self.triggered = true;
                        return true;
                    }
                }
            }

            updated
        } else {
            false
        }
    }

    /// Refresh iceberg visible quantity after fill
    pub fn refresh_iceberg(&mut self, filled_qty: Quantity) {
        if let OrderType::Iceberg(params) = &mut self.order_type {
            params.filled_qty = params.filled_qty.saturating_add(filled_qty);

            let remaining = params.total_qty.saturating_sub(params.filled_qty);
            if remaining > Quantity::ZERO {
                self.qty = Quantity::new(remaining.value().min(params.visible_qty.value()));
            } else {
                self.qty = Quantity::ZERO;
            }
        }
    }

    /// Check if post-only order would cross (take liquidity)
    pub fn would_cross(&self, best_bid: Option<Price>, best_ask: Option<Price>) -> bool {
        if !matches!(self.order_type, OrderType::PostOnly) {
            return false;
        }

        match self.side {
            Side::Buy => {
                // Buy order would cross if price >= best ask
                if let Some(ask) = best_ask {
                    self.price >= ask
                } else {
                    false
                }
            }
            Side::Sell => {
                // Sell order would cross if price <= best bid
                if let Some(bid) = best_bid {
                    self.price <= bid
                } else {
                    false
                }
            }
        }
    }

    /// Convert to basic Order (for execution)
    pub fn to_basic_order(&self) -> crate::book::Order {
        crate::book::Order {
            id: self.id,
            side: self.side,
            price: self.price,
            qty: self.qty,
            timestamp: self.timestamp,
            tif: self.tif,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stop_loss_trigger() {
        let order = AdvancedOrder::new_stop_loss(
            OrderId::new(1),
            Side::Sell,
            Quantity::new(100),
            Price::from_f64(50000.0),
            false,
            None,
            Timestamp::new(1000),
        );

        assert!(!order.check_stop_loss_trigger(Price::from_f64(50001.0)));
        assert!(order.check_stop_loss_trigger(Price::from_f64(50000.0)));
        assert!(order.check_stop_loss_trigger(Price::from_f64(49999.0)));
    }

    #[test]
    fn test_take_profit_trigger() {
        let order = AdvancedOrder::new_take_profit(
            OrderId::new(1),
            Side::Sell,
            Quantity::new(100),
            Price::from_f64(55000.0),
            Price::from_f64(55000.0),
            Timestamp::new(1000),
        );

        assert!(!order.check_take_profit_trigger(Price::from_f64(54999.0)));
        assert!(order.check_take_profit_trigger(Price::from_f64(55000.0)));
        assert!(order.check_take_profit_trigger(Price::from_f64(55001.0)));
    }

    #[test]
    fn test_trailing_stop_update() {
        let mut order = AdvancedOrder::new_trailing_stop(
            OrderId::new(1),
            Side::Sell,
            Quantity::new(100),
            Price::from_f64(1000.0),
            0,
            Price::from_f64(50000.0),
            Timestamp::new(1000),
        );

        // Market rises, stop should trail up
        assert!(order.update_trailing_stop(Price::from_f64(51000.0)));
        if let OrderType::TrailingStop(params) = &order.order_type {
            assert_eq!(params.stop_price, Price::from_f64(50000.0)); // 51000 - 1000
        }

        // Market continues to rise
        assert!(order.update_trailing_stop(Price::from_f64(52000.0)));
        if let OrderType::TrailingStop(params) = &order.order_type {
            assert_eq!(params.stop_price, Price::from_f64(51000.0)); // 52000 - 1000
        }

        // Market drops below stop, should trigger
        assert!(order.update_trailing_stop(Price::from_f64(50000.0)));
        assert!(order.triggered);
    }

    #[test]
    fn test_iceberg_refresh() {
        let mut order = AdvancedOrder::new_iceberg(
            OrderId::new(1),
            Side::Buy,
            Price::from_f64(50000.0),
            Quantity::new(1000),
            Quantity::new(100),
            Timestamp::new(1000),
        );

        assert_eq!(order.qty, Quantity::new(100));

        // Fill first slice
        order.refresh_iceberg(Quantity::new(100));
        assert_eq!(order.qty, Quantity::new(100)); // Next slice

        // Fill 9 more slices
        for _ in 0..9 {
            order.refresh_iceberg(Quantity::new(100));
        }
        assert_eq!(order.qty, Quantity::ZERO); // All filled
    }

    #[test]
    fn test_post_only_would_cross() {
        let order = AdvancedOrder::new_post_only(
            OrderId::new(1),
            Side::Buy,
            Price::from_f64(50000.0),
            Quantity::new(100),
            Timestamp::new(1000),
        );

        // Would cross if best ask is <= our price
        assert!(order.would_cross(Some(Price::from_f64(49900.0)), Some(Price::from_f64(50000.0))));
        assert!(order.would_cross(Some(Price::from_f64(49900.0)), Some(Price::from_f64(49999.0))));
        assert!(!order.would_cross(Some(Price::from_f64(49900.0)), Some(Price::from_f64(50001.0))));
    }
}
