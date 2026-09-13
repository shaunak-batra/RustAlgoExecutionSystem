//! Deterministic engine core.
//!
//! Every state change happens in an [`EngineCore`] method that takes the
//! current time as an argument, and no method reads a clock. Feeding the same
//! commands with the same timestamps produces the same child orders, fills and
//! P&L on a given build and platform (the Almgren-Chriss schedule uses libm
//! exponentials, which are not guaranteed bit-identical across platforms). That
//! is what makes the engine testable without a clock. The async wrapper in
//! `event_loop` is the part that reads the wall clock and passes it in.

use crate::accounting::{Position, TickValue};
use crate::risk::{RiskLimits, RiskViolation, SymbolExposure};
use crate::state::ParentOrder;
use crate::venue::{SimulatedVenue, SymbolConfig, VenueError};
use algo_core::{
    compute_almgren_chriss_schedule, compute_twap_schedule, compute_vwap_schedule,
    AlmgrenChrissParams, ChildOrderInstruction, ScheduleError, TwapParams, VwapParams,
};
use api::{
    AlgorithmSpec, FillReport, NewParentOrder, ParentOrderState, ParentOrderView, PositionView,
    PositionsSnapshot,
};
use orderbook::{Order, OrderId, Price, PriceError, Quantity, Side, TimeInForce, Timestamp};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;
use tracing::{error, warn};

/// Engine configuration, in engine units (ticks).
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub risk: RiskLimits,
    pub symbols: Vec<SymbolConfig>,
    /// Finished parent orders kept for status queries; older ones are dropped.
    pub retained_finished_orders: usize,
}

impl EngineConfig {
    /// Converts settings, whose money amounts are in price units, into ticks.
    pub fn from_settings(settings: &config::Settings) -> Result<Self, PriceError> {
        let ticks =
            |amount: f64| Price::try_from_f64(amount).map(|price| TickValue::from(price.ticks()));
        let risk = &settings.risk;
        let symbols = settings
            .venue
            .symbols
            .iter()
            .map(|symbol| {
                Ok(SymbolConfig {
                    symbol: symbol.symbol.clone(),
                    reference_price: Price::try_from_f64(symbol.reference_price)?,
                    levels: symbol.levels,
                    qty_per_level: symbol.qty_per_level,
                    level_spacing_bps: symbol.level_spacing_bps,
                })
            })
            .collect::<Result<_, PriceError>>()?;
        Ok(Self {
            risk: RiskLimits {
                max_order_qty: risk.max_order_qty,
                max_order_notional: ticks(risk.max_order_notional)?,
                max_position_qty: risk.max_position_qty,
                max_gross_notional: ticks(risk.max_gross_notional)?,
                price_collar_bps: risk.price_collar_bps,
                max_loss: ticks(risk.max_loss)?,
            },
            symbols,
            retained_finished_orders: settings.engine.retained_finished_orders,
        })
    }
}

/// Why a new parent order was not accepted.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RejectReason {
    #[error("trading is halted: {0}")]
    Halted(String),
    #[error("unknown symbol {0}")]
    UnknownSymbol(String),
    #[error("quantity must be greater than zero")]
    ZeroQuantity,
    #[error("limit price must be positive")]
    NonPositiveLimitPrice,
    #[error("the execution window has already ended")]
    WindowEnded,
    #[error("no two-sided market in {0}")]
    NoMarket(String),
    #[error("invalid schedule: {0}")]
    Schedule(#[from] ScheduleError),
    #[error("risk check failed: {0}")]
    Risk(#[from] RiskViolation),
}

/// Why a cancel request failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CancelError {
    #[error("no parent order {0}")]
    Unknown(u64),
    #[error("parent order {id} is {state:?}, not working")]
    NotWorking { id: u64, state: ParentOrderState },
}

/// All engine state: the simulated venue, parent orders, positions and the kill switch.
pub struct EngineCore {
    venue: SimulatedVenue,
    risk: RiskLimits,
    parents: BTreeMap<u64, ParentOrder>,
    working: BTreeSet<u64>,
    finished: VecDeque<u64>,
    retained_finished_orders: usize,
    positions: BTreeMap<String, Position>,
    next_parent_id: u64,
    next_child_id: u64,
    halt_reason: Option<String>,
}

impl EngineCore {
    pub fn new(config: EngineConfig) -> Result<Self, VenueError> {
        Ok(Self {
            venue: SimulatedVenue::new(config.symbols)?,
            risk: config.risk,
            parents: BTreeMap::new(),
            working: BTreeSet::new(),
            finished: VecDeque::new(),
            retained_finished_orders: config.retained_finished_orders,
            positions: BTreeMap::new(),
            next_parent_id: 1,
            next_child_id: 1,
            halt_reason: None,
        })
    }

    /// Validates, schedules and risk-checks a parent order, and returns its id.
    pub fn submit(&mut self, order: NewParentOrder, now_ns: u64) -> Result<u64, RejectReason> {
        if let Some(reason) = &self.halt_reason {
            return Err(RejectReason::Halted(reason.clone()));
        }
        if !self.venue.contains(&order.symbol) {
            return Err(RejectReason::UnknownSymbol(order.symbol));
        }
        if order.quantity == 0 {
            return Err(RejectReason::ZeroQuantity);
        }
        if order.limit_price.is_some_and(|price| price.ticks() <= 0) {
            return Err(RejectReason::NonPositiveLimitPrice);
        }
        if order.end_ns <= now_ns {
            return Err(RejectReason::WindowEnded);
        }
        let mid = self
            .venue
            .mid(&order.symbol)
            .ok_or_else(|| RejectReason::NoMarket(order.symbol.clone()))?;
        // Risk before the schedule: the checks are cheap, and a schedule can have
        // a million slices.
        self.risk.check_new_order(
            order.side,
            order.quantity,
            order.limit_price,
            self.exposure(&order.symbol, mid),
            self.gross_notional(),
        )?;
        let schedule = build_schedule(&order)?;

        let id = self.next_parent_id;
        self.next_parent_id += 1;
        self.parents
            .insert(id, ParentOrder::new(id, &order, schedule, mid));
        self.working.insert(id);
        Ok(id)
    }

    /// Stops a working parent order from releasing further child orders.
    pub fn cancel(&mut self, parent_order_id: u64) -> Result<(), CancelError> {
        let parent = self
            .parents
            .get_mut(&parent_order_id)
            .ok_or(CancelError::Unknown(parent_order_id))?;
        if !parent.is_working() {
            return Err(CancelError::NotWorking {
                id: parent_order_id,
                state: parent.state(),
            });
        }
        parent.cancel("cancelled by client");
        self.working.remove(&parent_order_id);
        self.retire(parent_order_id);
        Ok(())
    }

    /// Advances the engine to `now_ns`.
    ///
    /// For each working parent order in id order, sends the quantity now due as
    /// one IOC child order and applies the fills; an order whose window is over
    /// then finishes. Afterwards the simulated quotes are refreshed and the kill
    /// switch is checked. Returns the fills in execution order.
    ///
    /// Refreshing at the end rather than the start changes nothing about what
    /// children trade against, because nothing trades between ticks. What it
    /// changes is that the loss check, and every query and submission until the
    /// next tick, see a full ladder rather than one this tick's fills drained.
    pub fn tick(&mut self, now_ns: u64) -> Vec<FillReport> {
        let mut fills = Vec::new();
        let mut finished = Vec::new();
        let mut accounting_failure = None;

        for &id in &self.working {
            // After an accounting overflow no further child orders go out; the
            // halt at the end of the tick cancels whatever is left.
            if accounting_failure.is_some() {
                break;
            }
            let Some(parent) = self.parents.get_mut(&id) else {
                continue;
            };
            // A slice is released on the first tick at or after its time. On the
            // first tick at or after the window's end, that is every slice no
            // earlier tick reached, so they go out as one last child instead of
            // being dropped when ticks are coarser than the slices.
            let qty = parent.release_due(now_ns);
            if qty > 0 {
                let child_id = self.next_child_id;
                self.next_child_id += 1;
                parent.record_child(child_id, now_ns, qty);
                let order = child_order(child_id, parent.side, parent.limit_price, qty, now_ns);
                match self.venue.execute(&parent.symbol, order) {
                    Ok(result) => {
                        for fill in result.fills {
                            let filled = fill.qty.value();
                            // A position is created by its first fill, so a child
                            // that fills nothing leaves no empty row behind.
                            let position = self.positions.entry(parent.symbol.clone()).or_default();
                            if let Err(overflow) =
                                position.apply_fill(parent.side, fill.price, filled)
                            {
                                // Record nothing the position could not take, so the
                                // order, the fill stream and the position agree.
                                accounting_failure =
                                    Some(format!("{} position: {overflow}", parent.symbol));
                                break;
                            }
                            parent.record_fill(fill.price, filled);
                            fills.push(FillReport {
                                parent_order_id: id,
                                child_order_id: child_id,
                                symbol: parent.symbol.clone(),
                                side: parent.side,
                                price: fill.price,
                                quantity: filled,
                                timestamp_ns: now_ns,
                            });
                        }
                        parent.finish_if_filled();
                    }
                    Err(e) => error!(
                        parent_order_id = id,
                        child_order_id = child_id,
                        "child order failed: {e}"
                    ),
                }
            }
            if parent.is_working() && now_ns >= parent.end_ns {
                parent.finish_window();
            }
            if !parent.is_working() {
                finished.push(id);
            }
        }

        for id in finished {
            self.working.remove(&id);
            self.retire(id);
        }
        self.venue.refresh_quotes();
        match accounting_failure {
            Some(reason) => self.halt(reason),
            None => self.check_loss_limit(),
        }
        fills
    }

    /// Stops trading: cancels every working order and rejects new ones. A
    /// halted engine stays halted until it is restarted.
    pub fn halt(&mut self, reason: String) {
        warn!("trading halted: {reason}");
        for id in std::mem::take(&mut self.working) {
            if let Some(parent) = self.parents.get_mut(&id) {
                parent.cancel(format!("kill switch: {reason}"));
            }
            self.retire(id);
        }
        self.halt_reason = Some(reason);
    }

    pub fn halt_reason(&self) -> Option<&str> {
        self.halt_reason.as_deref()
    }

    pub fn order_status(&self, parent_order_id: u64) -> Option<ParentOrderView> {
        self.parents.get(&parent_order_id).map(ParentOrder::view)
    }

    /// Positions (all symbols, or one) with unrealized P&L at the current mid.
    pub fn positions(&self, symbol: Option<&str>) -> PositionsSnapshot {
        let positions = self
            .positions
            .iter()
            .filter(|(name, _)| symbol.is_none_or(|wanted| wanted == name.as_str()))
            .map(|(name, position)| {
                let mark = self.venue.mid(name);
                PositionView {
                    symbol: name.clone(),
                    quantity: position.quantity,
                    average_price_ticks: position.average_price_ticks(),
                    realized_pnl: position.realized_pnl,
                    unrealized_pnl: mark.map(|mark| position.unrealized_pnl(mark)),
                    mark_price: mark,
                }
            })
            .collect();
        PositionsSnapshot {
            positions,
            halt_reason: self.halt_reason.clone(),
        }
    }

    pub fn venue(&self) -> &SimulatedVenue {
        &self.venue
    }

    fn check_loss_limit(&mut self) {
        if self.halt_reason.is_some() {
            return;
        }
        let total = self.total_pnl();
        if self.risk.loss_limit_breached(total) {
            self.halt(format!(
                "total P&L {total} is at or below the loss limit of -{} (ticks x units)",
                self.risk.max_loss
            ));
        }
    }

    /// Realized plus unrealized P&L across symbols, marked with [`Self::mark`].
    fn total_pnl(&self) -> TickValue {
        self.positions
            .iter()
            .map(|(symbol, position)| {
                let unrealized = self
                    .mark(symbol)
                    .map_or(0, |mark| position.unrealized_pnl(mark));
                position.realized_pnl.saturating_add(unrealized)
            })
            .fold(0, TickValue::saturating_add)
    }

    fn exposure(&self, symbol: &str, mid: Price) -> SymbolExposure {
        let mut exposure = SymbolExposure {
            position: self.positions.get(symbol).map_or(0, |p| p.quantity),
            working_buy_qty: 0,
            working_sell_qty: 0,
            mid,
        };
        for parent in self
            .working
            .iter()
            .filter_map(|id| self.parents.get(id))
            .filter(|parent| parent.symbol == symbol)
        {
            let unfilled = parent.unfilled_qty();
            match parent.side {
                Side::Buy => {
                    exposure.working_buy_qty = exposure.working_buy_qty.saturating_add(unfilled)
                }
                Side::Sell => {
                    exposure.working_sell_qty = exposure.working_sell_qty.saturating_add(unfilled)
                }
            }
        }
        exposure
    }

    /// The price risk values a symbol at: the mid, or the reference price if a
    /// side of the book is empty. Quotes are refreshed at the end of every tick,
    /// so outside a tick the mid is always there; the fallback only stops a
    /// symbol dropping out of the risk totals if that ever stopped being true.
    fn mark(&self, symbol: &str) -> Option<Price> {
        self.venue
            .mid(symbol)
            .or_else(|| self.venue.reference_price(symbol))
    }

    /// Sum over symbols of (|position| + unfilled working quantity) × mark.
    fn gross_notional(&self) -> TickValue {
        self.venue
            .symbols()
            .filter_map(|symbol| {
                let mid = self.mark(symbol)?;
                let exposure = self.exposure(symbol, mid);
                let units = i128::from(exposure.position).abs()
                    + i128::from(exposure.working_buy_qty)
                    + i128::from(exposure.working_sell_qty);
                Some(units.saturating_mul(i128::from(mid.ticks())))
            })
            .fold(0, TickValue::saturating_add)
    }

    /// Moves a finished order into the retention queue, dropping the oldest
    /// finished orders beyond the retention limit.
    fn retire(&mut self, id: u64) {
        self.finished.push_back(id);
        while self.finished.len() > self.retained_finished_orders {
            if let Some(oldest) = self.finished.pop_front() {
                self.parents.remove(&oldest);
            }
        }
    }
}

fn build_schedule(order: &NewParentOrder) -> Result<Vec<ChildOrderInstruction>, ScheduleError> {
    match &order.algorithm {
        AlgorithmSpec::Twap { num_slices } => compute_twap_schedule(TwapParams {
            start_ns: order.start_ns,
            end_ns: order.end_ns,
            total_qty: order.quantity,
            num_slices: *num_slices,
        }),
        AlgorithmSpec::Vwap { volume_profile } => compute_vwap_schedule(
            VwapParams {
                start_ns: order.start_ns,
                end_ns: order.end_ns,
                total_qty: order.quantity,
            },
            volume_profile,
        ),
        AlgorithmSpec::ImplementationShortfall {
            num_slices,
            risk_aversion,
            volatility,
            temporary_impact,
            permanent_impact,
        } => compute_almgren_chriss_schedule(AlmgrenChrissParams {
            start_ns: order.start_ns,
            end_ns: order.end_ns,
            total_qty: order.quantity,
            num_slices: *num_slices,
            risk_aversion: *risk_aversion,
            volatility: *volatility,
            temporary_impact: *temporary_impact,
            permanent_impact: *permanent_impact,
        })
        .map(|schedule| schedule.children),
    }
}

/// An immediate-or-cancel child order, limited or at market.
fn child_order(id: u64, side: Side, limit: Option<Price>, qty: u64, now_ns: u64) -> Order {
    let id = OrderId::new(id);
    let qty = Quantity::new(qty);
    let timestamp = Timestamp::new(now_ns);
    let order = match limit {
        Some(price) => Order::limit(id, side, price, qty, timestamp),
        None => Order::market(id, side, qty, timestamp),
    };
    order.with_tif(TimeInForce::IOC)
}
