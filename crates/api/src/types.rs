//! Commands and views exchanged between the gRPC layer and the engine task.
//!
//! The engine owns all state in a single task. Callers send an
//! [`EngineCommand`] over a channel and receive the answer on the command's
//! oneshot `reply` channel.

use orderbook::{Price, Side};
use tokio::sync::{broadcast, oneshot};

/// How a parent order is split into child orders.
#[derive(Debug, Clone, PartialEq)]
pub enum AlgorithmSpec {
    /// Equal quantity per time slice.
    Twap { num_slices: usize },
    /// Quantity proportional to a volume profile (one non-negative weight per slice).
    Vwap { volume_profile: Vec<f64> },
    /// Almgren–Chriss implementation shortfall (see `algo_core::is`), time in seconds.
    ImplementationShortfall {
        num_slices: usize,
        risk_aversion: f64,
        volatility: f64,
        temporary_impact: f64,
        permanent_impact: f64,
    },
}

impl AlgorithmSpec {
    pub fn name(&self) -> &'static str {
        match self {
            AlgorithmSpec::Twap { .. } => "TWAP",
            AlgorithmSpec::Vwap { .. } => "VWAP",
            AlgorithmSpec::ImplementationShortfall { .. } => "IS",
        }
    }
}

/// A parent order as submitted by a client.
#[derive(Debug, Clone, PartialEq)]
pub struct NewParentOrder {
    pub symbol: String,
    pub side: Side,
    pub quantity: u64,
    /// Worst acceptable execution price; `None` sends market child orders.
    pub limit_price: Option<Price>,
    /// Execution window `[start_ns, end_ns)`, Unix epoch nanoseconds.
    pub start_ns: u64,
    pub end_ns: u64,
    pub algorithm: AlgorithmSpec,
}

/// Lifecycle state of an accepted parent order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentOrderState {
    /// Releasing child orders.
    Working,
    /// The full quantity executed.
    Filled,
    /// Stopped by a cancel request or by the kill switch.
    Cancelled,
    /// The window ended with quantity unfilled.
    Expired,
}

/// A child order the engine has sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildOrderView {
    pub child_order_id: u64,
    pub sent_at_ns: u64,
    /// Includes quantity carried over from earlier children that did not fill.
    pub quantity: u64,
    pub filled_quantity: u64,
}

/// Snapshot of a parent order.
#[derive(Debug, Clone, PartialEq)]
pub struct ParentOrderView {
    pub id: u64,
    pub symbol: String,
    pub side: Side,
    pub algorithm: &'static str,
    pub state: ParentOrderState,
    /// Why the order was cancelled or expired.
    pub state_reason: Option<String>,
    pub quantity: u64,
    pub filled_quantity: u64,
    pub limit_price: Option<Price>,
    pub start_ns: u64,
    pub end_ns: u64,
    /// Mid price when the order was accepted.
    pub arrival_mid: Price,
    /// Volume-weighted average fill price in ticks, if anything filled.
    pub average_fill_price_ticks: Option<f64>,
    /// Cost against the arrival mid in basis points, positive when worse
    /// (paid more on a buy, received less on a sell).
    pub shortfall_bps: Option<f64>,
    /// Scheduled child orders not yet released.
    pub pending_slices: usize,
    /// Child orders released so far, oldest first.
    pub children: Vec<ChildOrderView>,
}

/// Position and P&L in one symbol. Money is in price ticks × units.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionView {
    pub symbol: String,
    /// Positive when long, negative when short.
    pub quantity: i64,
    /// `None` when flat.
    pub average_price_ticks: Option<f64>,
    pub realized_pnl: i128,
    /// At `mark_price`; `None` without a two-sided market.
    pub unrealized_pnl: Option<i128>,
    pub mark_price: Option<Price>,
}

/// Every position, plus the kill switch state.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionsSnapshot {
    pub positions: Vec<PositionView>,
    /// Set once the kill switch has halted trading.
    pub halt_reason: Option<String>,
}

/// One fill of an engine child order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillReport {
    pub parent_order_id: u64,
    pub child_order_id: u64,
    pub symbol: String,
    pub side: Side,
    pub price: Price,
    pub quantity: u64,
    pub timestamp_ns: u64,
}

/// Commands handled by the engine task.
#[derive(Debug)]
pub enum EngineCommand {
    /// Replies with the new parent order id, or the rejection reason.
    SubmitParentOrder {
        order: NewParentOrder,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    /// Replies with why the order could not be cancelled, if it could not.
    CancelParentOrder {
        parent_order_id: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    GetOrderStatus {
        parent_order_id: u64,
        reply: oneshot::Sender<Option<ParentOrderView>>,
    },
    /// `symbol: None` returns every position.
    GetPositions {
        symbol: Option<String>,
        reply: oneshot::Sender<PositionsSnapshot>,
    },
    /// Replies with a receiver for fills from now on.
    SubscribeFills {
        reply: oneshot::Sender<broadcast::Receiver<FillReport>>,
    },
    Shutdown,
}
