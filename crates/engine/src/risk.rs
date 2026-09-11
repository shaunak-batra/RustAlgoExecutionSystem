//! Pre-trade risk limits and the loss kill switch.

use crate::accounting::TickValue;
use orderbook::{Price, Side};
use thiserror::Error;

/// Risk limits. Notional and P&L amounts are in price ticks × units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskLimits {
    /// Largest quantity of a single parent order.
    pub max_order_qty: u64,
    /// Largest notional of a single parent order, priced at the worse (higher)
    /// of its limit price and the mid.
    pub max_order_notional: TickValue,
    /// Largest absolute position per symbol, assuming every working order fills.
    pub max_position_qty: u64,
    /// Largest gross exposure across symbols, including the new order:
    /// the sum of (|position| + unfilled working quantity) × mid.
    pub max_gross_notional: TickValue,
    /// A limit price may differ from the mid by at most this many basis points.
    pub price_collar_bps: u32,
    /// Trading halts once total P&L (realized plus unrealized at the mid) is
    /// at or below `-max_loss`.
    pub max_loss: TickValue,
}

/// Exposure in one symbol, as used by the pre-trade checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolExposure {
    pub position: i64,
    /// Unfilled quantity of working buy orders.
    pub working_buy_qty: u64,
    /// Unfilled quantity of working sell orders.
    pub working_sell_qty: u64,
    pub mid: Price,
}

/// Why a new order failed a pre-trade check.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RiskViolation {
    #[error("quantity {qty} exceeds the per-order limit of {limit}")]
    OrderQuantity { qty: u64, limit: u64 },
    #[error("limit price {limit_price} is more than {collar_bps} bps from the mid {mid}")]
    PriceCollar {
        limit_price: Price,
        mid: Price,
        collar_bps: u32,
    },
    #[error("notional {notional} exceeds the per-order limit of {limit} (ticks x units)")]
    OrderNotional {
        notional: TickValue,
        limit: TickValue,
    },
    #[error("the position could reach {worst_case}, beyond the limit of {limit} either way")]
    PositionLimit { worst_case: i128, limit: u64 },
    #[error("gross exposure could reach {gross}, beyond the limit of {limit} (ticks x units)")]
    GrossNotional { gross: TickValue, limit: TickValue },
}

impl RiskLimits {
    /// Pre-trade checks for a new parent order. `exposure` is the order's
    /// symbol; `gross_notional` is the current gross exposure across all
    /// symbols, before this order.
    pub fn check_new_order(
        &self,
        side: Side,
        qty: u64,
        limit_price: Option<Price>,
        exposure: SymbolExposure,
        gross_notional: TickValue,
    ) -> Result<(), RiskViolation> {
        if qty > self.max_order_qty {
            return Err(RiskViolation::OrderQuantity {
                qty,
                limit: self.max_order_qty,
            });
        }

        let mid = TickValue::from(exposure.mid.ticks());
        let qty_value = TickValue::from(qty);

        if let Some(limit_price) = limit_price {
            let distance = (TickValue::from(limit_price.ticks()) - mid).abs();
            if distance * 10_000 > TickValue::from(self.price_collar_bps) * mid {
                return Err(RiskViolation::PriceCollar {
                    limit_price,
                    mid: exposure.mid,
                    collar_bps: self.price_collar_bps,
                });
            }
        }

        let worst_price = limit_price.map_or(mid, |price| TickValue::from(price.ticks()).max(mid));
        let notional = qty_value.saturating_mul(worst_price);
        if notional > self.max_order_notional {
            return Err(RiskViolation::OrderNotional {
                notional,
                limit: self.max_order_notional,
            });
        }

        let position = i128::from(exposure.position);
        let worst_case = match side {
            Side::Buy => position + i128::from(exposure.working_buy_qty) + qty_value,
            Side::Sell => position - i128::from(exposure.working_sell_qty) - qty_value,
        };
        if worst_case.abs() > i128::from(self.max_position_qty) {
            return Err(RiskViolation::PositionLimit {
                worst_case,
                limit: self.max_position_qty,
            });
        }

        let gross = gross_notional.saturating_add(qty_value.saturating_mul(mid));
        if gross > self.max_gross_notional {
            return Err(RiskViolation::GrossNotional {
                gross,
                limit: self.max_gross_notional,
            });
        }

        Ok(())
    }

    /// Whether total P&L has reached the loss limit.
    pub fn loss_limit_breached(&self, total_pnl: TickValue) -> bool {
        total_pnl <= -self.max_loss
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MID: i64 = 10_000_000;

    fn limits() -> RiskLimits {
        RiskLimits {
            max_order_qty: 1_000,
            max_order_notional: 1_000 * TickValue::from(MID),
            max_position_qty: 1_500,
            max_gross_notional: 5_000 * TickValue::from(MID),
            price_collar_bps: 100,
            max_loss: 1_000_000,
        }
    }

    fn flat() -> SymbolExposure {
        SymbolExposure {
            position: 0,
            working_buy_qty: 0,
            working_sell_qty: 0,
            mid: Price::new(MID),
        }
    }

    fn check(
        side: Side,
        qty: u64,
        limit: Option<i64>,
        exposure: SymbolExposure,
    ) -> Result<(), RiskViolation> {
        limits().check_new_order(side, qty, limit.map(Price::new), exposure, 0)
    }

    #[test]
    fn order_quantity_limit() {
        assert!(check(Side::Buy, 1_000, None, flat()).is_ok());
        assert!(matches!(
            check(Side::Buy, 1_001, None, flat()),
            Err(RiskViolation::OrderQuantity { .. })
        ));
    }

    #[test]
    fn price_collar_is_inclusive_on_both_sides() {
        // 100 bps of 10_000_000 ticks is 100_000 ticks.
        assert!(check(Side::Sell, 10, Some(MID + 100_000), flat()).is_ok());
        assert!(check(Side::Buy, 10, Some(MID - 100_000), flat()).is_ok());
        assert!(matches!(
            check(Side::Buy, 10, Some(MID + 100_001), flat()),
            Err(RiskViolation::PriceCollar { .. })
        ));
        assert!(matches!(
            check(Side::Sell, 10, Some(MID - 100_001), flat()),
            Err(RiskViolation::PriceCollar { .. })
        ));
    }

    #[test]
    fn notional_uses_the_worse_of_limit_and_mid() {
        assert!(check(Side::Buy, 1_000, None, flat()).is_ok());
        // A limit above the mid is priced at the limit...
        assert!(matches!(
            check(Side::Buy, 1_000, Some(MID + 1), flat()),
            Err(RiskViolation::OrderNotional { .. })
        ));
        // ...and a limit below the mid at the mid.
        assert!(check(Side::Sell, 1_000, Some(MID - 1), flat()).is_ok());
    }

    #[test]
    fn position_limit_counts_working_orders() {
        let long = SymbolExposure {
            position: 500,
            working_buy_qty: 600,
            ..flat()
        };
        assert!(check(Side::Buy, 400, None, long).is_ok());
        assert!(matches!(
            check(Side::Buy, 401, None, long),
            Err(RiskViolation::PositionLimit {
                worst_case: 1_501,
                ..
            })
        ));
        // Selling reduces the long, so it is allowed.
        assert!(check(Side::Sell, 1_000, None, long).is_ok());

        let short = SymbolExposure {
            position: -500,
            working_sell_qty: 600,
            ..flat()
        };
        assert!(check(Side::Sell, 400, None, short).is_ok());
        assert!(check(Side::Sell, 401, None, short).is_err());
    }

    #[test]
    fn gross_notional_includes_the_new_order() {
        let before = 4_500 * TickValue::from(MID);
        assert!(limits()
            .check_new_order(Side::Buy, 500, None, flat(), before)
            .is_ok());
        assert!(matches!(
            limits().check_new_order(Side::Buy, 501, None, flat(), before),
            Err(RiskViolation::GrossNotional { .. })
        ));
    }

    #[test]
    fn loss_limit_is_inclusive() {
        assert!(limits().loss_limit_breached(-1_000_000));
        assert!(!limits().loss_limit_breached(-999_999));
    }
}
