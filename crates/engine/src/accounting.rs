//! Exact position and P&L accounting.

use orderbook::{Price, Side};
use thiserror::Error;

/// Money in price ticks × quantity units. `i128` holds any `u64` quantity
/// times any `i64` price.
pub type TickValue = i128;

/// Applying a fill would overflow the position or P&L.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("position or P&L arithmetic would overflow")]
pub struct AccountingOverflow;

/// A signed position in one symbol, with average-cost accounting in exact integers.
///
/// After every fill, `realized_pnl - cost_basis` equals the net cash flow of
/// all fills so far (sales positive, purchases negative). So whenever the
/// position is flat, realized P&L is exactly the net cash of its trades, with
/// no floating-point drift whatever the sequence of fills.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    /// Positive when long, negative when short.
    pub quantity: i64,
    /// Cost of the open quantity: positive for a long, negative for a short,
    /// zero when flat.
    pub cost_basis: TickValue,
    pub realized_pnl: TickValue,
}

impl Position {
    /// Applies a fill. A fill against the position's direction first closes
    /// open quantity, realizing P&L against its average cost; any excess opens
    /// a position in the other direction. On overflow the position is unchanged.
    pub fn apply_fill(
        &mut self,
        side: Side,
        price: Price,
        qty: u64,
    ) -> Result<(), AccountingOverflow> {
        let mut next = *self;
        next.apply(side, price, qty).ok_or(AccountingOverflow)?;
        *self = next;
        Ok(())
    }

    fn apply(&mut self, side: Side, price: Price, qty: u64) -> Option<()> {
        let price = TickValue::from(price.ticks());
        let direction: i128 = match side {
            Side::Buy => 1,
            Side::Sell => -1,
        };
        let mut quantity = i128::from(self.quantity);
        let mut remaining = i128::from(qty);

        if quantity != 0 && quantity.signum() != direction {
            let open = quantity.abs();
            let closing = remaining.min(open);
            // The closed share of the cost basis. Truncation leaves any residue
            // with the open quantity; a full close removes the whole basis.
            let removed_cost = if closing == open {
                self.cost_basis
            } else {
                self.cost_basis.checked_mul(closing)? / open
            };
            let closing_value = quantity.signum().checked_mul(closing)?.checked_mul(price)?;
            self.realized_pnl = self
                .realized_pnl
                .checked_add(closing_value.checked_sub(removed_cost)?)?;
            self.cost_basis -= removed_cost;
            quantity += direction * closing;
            remaining -= closing;
        }

        if remaining > 0 {
            quantity = quantity.checked_add(direction * remaining)?;
            self.cost_basis = self
                .cost_basis
                .checked_add(direction.checked_mul(remaining)?.checked_mul(price)?)?;
        }

        self.quantity = i64::try_from(quantity).ok()?;
        Some(())
    }

    /// Average price of the open quantity in ticks, or `None` when flat.
    pub fn average_price_ticks(&self) -> Option<f64> {
        (self.quantity != 0).then(|| self.cost_basis as f64 / self.quantity as f64)
    }

    /// P&L of the open quantity marked at `mark`, saturating at the `i128` range.
    pub fn unrealized_pnl(&self, mark: Price) -> TickValue {
        (TickValue::from(self.quantity) * TickValue::from(mark.ticks()))
            .saturating_sub(self.cost_basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fill(position: &mut Position, side: Side, price: i64, qty: u64) {
        position.apply_fill(side, Price::new(price), qty).unwrap();
    }

    #[test]
    fn adding_to_a_long_averages_the_cost() {
        let mut position = Position::default();
        fill(&mut position, Side::Buy, 100, 10);
        fill(&mut position, Side::Buy, 105, 10);
        assert_eq!(
            position,
            Position {
                quantity: 20,
                cost_basis: 2_050,
                realized_pnl: 0
            }
        );
        assert_eq!(position.average_price_ticks(), Some(102.5));
    }

    #[test]
    fn round_trips_realize_exactly() {
        let mut long = Position::default();
        fill(&mut long, Side::Buy, 100, 10);
        fill(&mut long, Side::Sell, 110, 10);
        assert_eq!(
            long,
            Position {
                quantity: 0,
                cost_basis: 0,
                realized_pnl: 100
            }
        );

        let mut short = Position::default();
        fill(&mut short, Side::Sell, 110, 10);
        fill(&mut short, Side::Buy, 100, 10);
        assert_eq!(
            short,
            Position {
                quantity: 0,
                cost_basis: 0,
                realized_pnl: 100
            }
        );
    }

    #[test]
    fn partial_close_realizes_against_the_average_cost() {
        let mut position = Position::default();
        fill(&mut position, Side::Buy, 100, 3);
        fill(&mut position, Side::Buy, 104, 1); // 4 at an average of 101
        fill(&mut position, Side::Sell, 110, 2); // realizes 2 * (110 - 101)
        assert_eq!(
            position,
            Position {
                quantity: 2,
                cost_basis: 202,
                realized_pnl: 18
            }
        );
    }

    #[test]
    fn crossing_through_zero_flips_the_position() {
        // Short 5 at 100, then buy 10 at 90: covering 5 realizes +50, and the
        // other 5 open a long at 90.
        let mut position = Position::default();
        fill(&mut position, Side::Sell, 100, 5);
        fill(&mut position, Side::Buy, 90, 10);
        assert_eq!(
            position,
            Position {
                quantity: 5,
                cost_basis: 450,
                realized_pnl: 50
            }
        );
        assert_eq!(position.average_price_ticks(), Some(90.0));
    }

    #[test]
    fn unrealized_pnl_marks_the_open_quantity() {
        let mut long = Position::default();
        fill(&mut long, Side::Buy, 100, 10);
        assert_eq!(long.unrealized_pnl(Price::new(105)), 50);

        let mut short = Position::default();
        fill(&mut short, Side::Sell, 100, 10);
        assert_eq!(short.unrealized_pnl(Price::new(105)), -50);

        assert_eq!(Position::default().average_price_ticks(), None);
        assert_eq!(Position::default().unrealized_pnl(Price::new(105)), 0);
    }

    #[test]
    fn overflow_is_reported_and_leaves_the_position_unchanged() {
        let mut position = Position::default();
        fill(&mut position, Side::Buy, 1, i64::MAX as u64);
        let before = position;
        assert_eq!(
            position.apply_fill(Side::Buy, Price::new(1), 1),
            Err(AccountingOverflow)
        );
        assert_eq!(position, before);
    }

    proptest! {
        #[test]
        fn realized_minus_cost_basis_is_net_cash(
            fills in prop::collection::vec((any::<bool>(), 1i64..10_000_000, 1u64..1_000_000), 1..200),
        ) {
            let mut position = Position::default();
            let mut net_cash = 0i128;
            let mut net_quantity = 0i128;

            for (buy, price, qty) in fills {
                let side = if buy { Side::Buy } else { Side::Sell };
                position.apply_fill(side, Price::new(price), qty).unwrap();
                let signed_qty = if buy { i128::from(qty) } else { -i128::from(qty) };
                net_cash -= signed_qty * i128::from(price);
                net_quantity += signed_qty;

                prop_assert_eq!(i128::from(position.quantity), net_quantity);
                prop_assert_eq!(position.realized_pnl - position.cost_basis, net_cash);
                // Positive prices keep the cost basis on the same side as the position.
                prop_assert_eq!(position.cost_basis.signum(), i128::from(position.quantity.signum()));
                if position.quantity == 0 {
                    prop_assert_eq!(position.realized_pnl, net_cash);
                }
            }
        }
    }
}
