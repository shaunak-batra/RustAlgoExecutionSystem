use crate::state::EngineState;
use api::ParentOrder;
use orderbook::{Price, Side};
use thiserror::Error;

// Risk limit constants
const DEFAULT_MAX_POSITION: u64 = 1_000_000;
const DEFAULT_MAX_ORDER_SIZE: u64 = 100_000;
const DEFAULT_MAX_ORDER_NOTIONAL: f64 = 10_000_000.0;
const DEFAULT_MAX_TOTAL_NOTIONAL: f64 = 50_000_000.0;
const DEFAULT_MIN_PRICE: f64 = 0.01;
const DEFAULT_MAX_PRICE: f64 = 1_000_000.0;

/// Risk check errors.
#[derive(Debug, Error)]
pub enum RiskError {
    #[error("Position limit exceeded: current={current}, limit={limit}")]
    PositionLimitExceeded { current: i64, limit: i64 },

    #[error("Notional limit exceeded: value={value:.2}, limit={limit:.2}")]
    NotionalLimitExceeded { value: f64, limit: f64 },

    #[error("Order size too large: size={size}, max={max}")]
    OrderSizeTooLarge { size: u64, max: u64 },

    #[error("Invalid price: {reason}")]
    InvalidPrice { reason: String },

    #[error("Market closed or outside trading hours")]
    MarketClosed,

    #[error("Insufficient capital")]
    InsufficientCapital,
}

/// Risk configuration.
#[derive(Debug, Clone)]
pub struct RiskConfig {
    /// Maximum absolute position size per symbol
    pub max_position: u64,

    /// Maximum notional value per order (in dollars)
    pub max_order_notional: f64,

    /// Maximum single order size
    pub max_order_size: u64,

    /// Maximum total notional exposure across all positions
    pub max_total_notional: f64,

    /// Minimum price (for sanity checks)
    pub min_price: Price,

    /// Maximum price (for sanity checks)
    pub max_price: Price,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_position: DEFAULT_MAX_POSITION,
            max_order_notional: DEFAULT_MAX_ORDER_NOTIONAL,
            max_order_size: DEFAULT_MAX_ORDER_SIZE,
            max_total_notional: DEFAULT_MAX_TOTAL_NOTIONAL,
            min_price: Price::from_f64(DEFAULT_MIN_PRICE),
            max_price: Price::from_f64(DEFAULT_MAX_PRICE),
        }
    }
}

pub struct RiskChecker {
    config: RiskConfig,
}

impl RiskChecker {
    pub fn new(config: RiskConfig) -> Self {
        Self { config }
    }

    /// Pre-trade risk checks for a parent order.
    pub fn check_parent_order(
        &self,
        order: &ParentOrder,
        state: &EngineState,
    ) -> Result<(), RiskError> {
        // Check order size
        if order.total_qty.value() > self.config.max_order_size {
            return Err(RiskError::OrderSizeTooLarge {
                size: order.total_qty.value(),
                max: self.config.max_order_size,
            });
        }

        // Check notional value
        if let Some(price) = order.limit_price {
            let notional = price.as_f64() * order.total_qty.value() as f64;
            if notional > self.config.max_order_notional {
                return Err(RiskError::NotionalLimitExceeded {
                    value: notional,
                    limit: self.config.max_order_notional,
                });
            }

            // Check price sanity
            if price < self.config.min_price || price > self.config.max_price {
                return Err(RiskError::InvalidPrice {
                    reason: format!(
                        "Price {} is outside allowed range [{}, {}]",
                        price, self.config.min_price, self.config.max_price
                    ),
                });
            }
        }

        // Check position limit
        if let Some(current_pos) = state.get_position(&order.symbol) {
            let new_pos = match order.side {
                Side::Buy => current_pos.quantity + order.total_qty.value() as i64,
                Side::Sell => current_pos.quantity - order.total_qty.value() as i64,
            };

            if new_pos.abs() > self.config.max_position as i64 {
                return Err(RiskError::PositionLimitExceeded {
                    current: new_pos,
                    limit: self.config.max_position as i64,
                });
            }
        } else {
            // No existing position, check new position against limit
            if order.total_qty.value() > self.config.max_position {
                return Err(RiskError::PositionLimitExceeded {
                    current: order.total_qty.value() as i64,
                    limit: self.config.max_position as i64,
                });
            }
        }

        // Check total notional exposure
        let total_notional: f64 = state
            .positions
            .values()
            .map(|p| p.avg_price * p.quantity.abs() as f64)
            .sum();

        if total_notional > self.config.max_total_notional {
            return Err(RiskError::NotionalLimitExceeded {
                value: total_notional,
                limit: self.config.max_total_notional,
            });
        }

        Ok(())
    }

    /// Post-trade checks (e.g., after fills)
    pub fn check_post_trade(&self, state: &EngineState) -> Result<(), RiskError> {
        // Check total notional exposure across all positions
        let total_notional: f64 = state
            .positions
            .values()
            .map(|p| p.avg_price * p.quantity.abs() as f64)
            .sum();

        if total_notional > self.config.max_total_notional {
            return Err(RiskError::NotionalLimitExceeded {
                value: total_notional,
                limit: self.config.max_total_notional,
            });
        }

        // Check individual position limits
        for (_symbol, position) in &state.positions {
            if position.quantity.unsigned_abs() > self.config.max_position {
                return Err(RiskError::PositionLimitExceeded {
                    current: position.quantity,
                    limit: self.config.max_position as i64,
                });
            }

            // Check for excessive realized losses (circuit breaker)
            // Halt trading if losses exceed a threshold (e.g., 50% of max notional)
            let max_loss_threshold = -0.5 * self.config.max_total_notional;
            if position.realized_pnl < max_loss_threshold {
                return Err(RiskError::InsufficientCapital);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{ParentOrderStatus, Position};
    use orderbook::{Quantity, Timestamp};

    fn create_test_parent_order(
        symbol: &str,
        side: Side,
        qty: u64,
        price: Option<f64>,
    ) -> ParentOrder {
        ParentOrder {
            id: 1,
            symbol: symbol.to_string(),
            side,
            total_qty: Quantity::new(qty),
            filled_qty: Quantity::ZERO,
            limit_price: price.map(Price::from_f64),
            start_time_ns: 0,
            end_time_ns: 10_000,
            algo_type: "TWAP".to_string(),
            num_slices: 10,
            status: ParentOrderStatus::Pending,
            child_order_ids: vec![],
            created_at: Timestamp::now_nanos(),
        }
    }

    #[test]
    fn test_order_size_limit() {
        let config = RiskConfig {
            max_order_size: 1000,
            ..Default::default()
        };
        let checker = RiskChecker::new(config);
        let state = EngineState::new();

        // Order within limit
        let order = create_test_parent_order("BTC", Side::Buy, 500, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_ok());

        // Order exceeds limit
        let order = create_test_parent_order("BTC", Side::Buy, 2000, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_err());
    }

    #[test]
    fn test_notional_limit() {
        let config = RiskConfig {
            max_order_notional: 10_000.0,
            ..Default::default()
        };
        let checker = RiskChecker::new(config);
        let state = EngineState::new();

        // Within limit: 50 * 100 = 5000
        let order = create_test_parent_order("BTC", Side::Buy, 50, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_ok());

        // Exceeds limit: 200 * 100 = 20000
        let order = create_test_parent_order("BTC", Side::Buy, 200, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_err());
    }

    #[test]
    fn test_position_limit() {
        let config = RiskConfig {
            max_position: 100,
            ..Default::default()
        };
        let checker = RiskChecker::new(config);
        let mut state = EngineState::new();

        // Add existing position
        state.positions.insert(
            "BTC".to_string(),
            Position {
                symbol: "BTC".to_string(),
                quantity: 80,
                avg_price: 100.0,
                realized_pnl: 0.0,
                unrealized_pnl: 0.0,
            },
        );

        // Buy 10 more: 80 + 10 = 90 (OK)
        let order = create_test_parent_order("BTC", Side::Buy, 10, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_ok());

        // Buy 30 more: 80 + 30 = 110 (exceeds limit)
        let order = create_test_parent_order("BTC", Side::Buy, 30, Some(100.0));
        assert!(checker.check_parent_order(&order, &state).is_err());
    }

    #[test]
    fn test_price_sanity() {
        let config = RiskConfig {
            min_price: Price::from_f64(1.0),
            max_price: Price::from_f64(1000.0),
            ..Default::default()
        };
        let checker = RiskChecker::new(config);
        let state = EngineState::new();

        // Valid price
        let order = create_test_parent_order("BTC", Side::Buy, 10, Some(500.0));
        assert!(checker.check_parent_order(&order, &state).is_ok());

        // Price too low
        let order = create_test_parent_order("BTC", Side::Buy, 10, Some(0.5));
        assert!(checker.check_parent_order(&order, &state).is_err());

        // Price too high
        let order = create_test_parent_order("BTC", Side::Buy, 10, Some(5000.0));
        assert!(checker.check_parent_order(&order, &state).is_err());
    }
}
