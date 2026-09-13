//! Parent order lifecycle.

use algo_core::ChildOrderInstruction;
use api::{ChildOrderView, NewParentOrder, ParentOrderState, ParentOrderView};
use orderbook::{Price, Side};

/// A parent order being worked by the engine.
///
/// The schedule decides how much should have been released by each time. Each
/// time at least one slice becomes due, the engine sends one IOC child order
/// for everything released but not yet filled, so quantity an earlier child
/// could not fill is carried into the next one.
#[derive(Debug, Clone)]
pub struct ParentOrder {
    pub id: u64,
    pub symbol: String,
    pub side: Side,
    pub algorithm: &'static str,
    pub quantity: u64,
    pub limit_price: Option<Price>,
    pub start_ns: u64,
    pub end_ns: u64,
    /// Mid price when the order was accepted.
    pub arrival_mid: Price,
    state: ParentOrderState,
    state_reason: Option<String>,
    schedule: Vec<ChildOrderInstruction>,
    next_slice: usize,
    released_qty: u64,
    filled_qty: u64,
    /// Sum of fill price × quantity, in ticks × units.
    fill_notional: i128,
    children: Vec<ChildOrderView>,
}

impl ParentOrder {
    pub fn new(
        id: u64,
        order: &NewParentOrder,
        schedule: Vec<ChildOrderInstruction>,
        arrival_mid: Price,
    ) -> Self {
        Self {
            id,
            symbol: order.symbol.clone(),
            side: order.side,
            algorithm: order.algorithm.name(),
            quantity: order.quantity,
            limit_price: order.limit_price,
            start_ns: order.start_ns,
            end_ns: order.end_ns,
            arrival_mid,
            state: ParentOrderState::Working,
            state_reason: None,
            schedule,
            next_slice: 0,
            released_qty: 0,
            filled_qty: 0,
            fill_notional: 0,
            children: Vec::new(),
        }
    }

    pub fn state(&self) -> ParentOrderState {
        self.state
    }

    pub fn is_working(&self) -> bool {
        self.state == ParentOrderState::Working
    }

    pub fn filled_qty(&self) -> u64 {
        self.filled_qty
    }

    /// Quantity not yet filled.
    pub fn unfilled_qty(&self) -> u64 {
        self.quantity - self.filled_qty
    }

    /// Releases every scheduled slice due by `now_ns`. If at least one slice
    /// became due, returns the quantity to send now: everything released but
    /// not yet filled. Otherwise returns 0.
    pub fn release_due(&mut self, now_ns: u64) -> u64 {
        let mut released_any = false;
        while let Some(slice) = self.schedule.get(self.next_slice) {
            if slice.target_time_ns > now_ns {
                break;
            }
            self.released_qty += slice.qty;
            self.next_slice += 1;
            released_any = true;
        }
        if released_any {
            self.released_qty - self.filled_qty
        } else {
            0
        }
    }

    /// Records a child order that is about to be sent.
    pub fn record_child(&mut self, child_order_id: u64, sent_at_ns: u64, quantity: u64) {
        self.children.push(ChildOrderView {
            child_order_id,
            sent_at_ns,
            quantity,
            filled_quantity: 0,
        });
    }

    /// Records a fill of the most recent child order.
    pub fn record_fill(&mut self, price: Price, qty: u64) {
        self.filled_qty += qty;
        // Saturates rather than overflowing, which would take over 2^127 ticks x units.
        self.fill_notional = self
            .fill_notional
            .saturating_add(i128::from(price.ticks()) * i128::from(qty));
        if let Some(child) = self.children.last_mut() {
            child.filled_quantity += qty;
        }
    }

    /// Marks the order filled once its full quantity has executed.
    pub fn finish_if_filled(&mut self) {
        if self.filled_qty == self.quantity {
            self.state = ParentOrderState::Filled;
        }
    }

    /// Ends the order when its window is over: filled, or expired with the
    /// unfilled quantity in the reason.
    pub fn finish_window(&mut self) {
        if self.filled_qty == self.quantity {
            self.state = ParentOrderState::Filled;
        } else {
            self.state = ParentOrderState::Expired;
            self.state_reason = Some(format!(
                "window ended with {} of {} unfilled",
                self.unfilled_qty(),
                self.quantity
            ));
        }
    }

    pub fn cancel(&mut self, reason: impl Into<String>) {
        self.state = ParentOrderState::Cancelled;
        self.state_reason = Some(reason.into());
    }

    pub fn view(&self) -> ParentOrderView {
        let average_fill_price_ticks =
            (self.filled_qty > 0).then(|| self.fill_notional as f64 / self.filled_qty as f64);
        let shortfall_bps = average_fill_price_ticks.map(|average| {
            let arrival = self.arrival_mid.ticks() as f64;
            let direction = match self.side {
                Side::Buy => 1.0,
                Side::Sell => -1.0,
            };
            direction * (average - arrival) / arrival * 10_000.0
        });
        ParentOrderView {
            id: self.id,
            symbol: self.symbol.clone(),
            side: self.side,
            algorithm: self.algorithm,
            state: self.state,
            state_reason: self.state_reason.clone(),
            quantity: self.quantity,
            filled_quantity: self.filled_qty,
            limit_price: self.limit_price,
            start_ns: self.start_ns,
            end_ns: self.end_ns,
            arrival_mid: self.arrival_mid,
            average_fill_price_ticks,
            shortfall_bps,
            pending_slices: self.schedule.len() - self.next_slice,
            children: self.children.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::AlgorithmSpec;

    const SECOND: u64 = 1_000_000_000;
    const MID: i64 = 10_000_000;

    fn parent(side: Side) -> ParentOrder {
        let order = NewParentOrder {
            symbol: "SIM".to_string(),
            side,
            quantity: 300,
            limit_price: None,
            start_ns: 0,
            end_ns: 3 * SECOND,
            algorithm: AlgorithmSpec::Twap { num_slices: 3 },
        };
        let schedule = (0..3)
            .map(|k| ChildOrderInstruction {
                target_time_ns: k * SECOND,
                qty: 100,
            })
            .collect();
        ParentOrder::new(7, &order, schedule, Price::new(MID))
    }

    #[test]
    fn due_slices_are_merged_and_shortfalls_carried_forward() {
        let mut order = parent(Side::Buy);
        assert_eq!(order.release_due(0), 100);
        order.record_child(1, 0, 100);
        order.record_fill(Price::new(MID), 60);

        assert_eq!(order.release_due(SECOND / 2), 0, "no new slice is due yet");
        // Two slices come due at once, plus the 40 the first child missed.
        assert_eq!(order.release_due(2 * SECOND), 240);
        assert_eq!(order.view().pending_slices, 0);
    }

    #[test]
    fn window_end_fills_or_expires() {
        let mut expired = parent(Side::Buy);
        expired.release_due(2 * SECOND);
        expired.record_child(1, 2 * SECOND, 300);
        expired.record_fill(Price::new(MID), 180);
        expired.finish_if_filled();
        assert!(expired.is_working());
        expired.finish_window();
        assert_eq!(expired.state(), ParentOrderState::Expired);
        assert_eq!(
            expired.view().state_reason.as_deref(),
            Some("window ended with 120 of 300 unfilled")
        );

        let mut filled = parent(Side::Buy);
        filled.release_due(2 * SECOND);
        filled.record_child(1, 2 * SECOND, 300);
        filled.record_fill(Price::new(MID), 300);
        filled.finish_if_filled();
        assert_eq!(filled.state(), ParentOrderState::Filled);
    }

    #[test]
    fn view_reports_average_price_and_signed_shortfall() {
        let mut buy = parent(Side::Buy);
        buy.release_due(0);
        buy.record_child(1, 0, 100);
        buy.record_fill(Price::new(MID + 1_000), 50);
        buy.record_fill(Price::new(MID + 3_000), 50);
        let view = buy.view();
        assert_eq!(view.average_fill_price_ticks, Some((MID + 2_000) as f64));
        assert!((view.shortfall_bps.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(view.children[0].filled_quantity, 100);

        let mut sell = parent(Side::Sell);
        sell.release_due(0);
        sell.record_child(1, 0, 100);
        sell.record_fill(Price::new(MID - 1_000), 100);
        assert!((sell.view().shortfall_bps.unwrap() - 1.0).abs() < 1e-9);

        assert_eq!(parent(Side::Buy).view().shortfall_bps, None);
    }
}
