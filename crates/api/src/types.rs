use orderbook::{Fill, OrderId, Price, Quantity, Side, Timestamp};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};

#[derive(Debug)]
pub enum EngineCommand {
    SubmitParentOrder {
        parent_id: u64,
        symbol: String,
        side: Side,
        qty: u64,
        limit_price: Option<Price>,
        start_ns: u64,
        end_ns: u64,
        num_slices: usize,
    },

    QueryStatus {
        parent_id: u64,
        response_tx: oneshot::Sender<Option<ParentOrder>>,
    },

    GetPositions {
        symbol: Option<String>,
        response_tx: oneshot::Sender<Vec<Position>>,
    },

    SubscribeToFills {
        response_tx: oneshot::Sender<broadcast::Receiver<Fill>>,
    },

    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentOrderStatus {
    Pending,
    Working,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentOrder {
    pub id: u64,
    pub symbol: String,
    pub side: Side,
    pub total_qty: Quantity,
    pub filled_qty: Quantity,
    pub limit_price: Option<Price>,
    pub start_time_ns: u64,
    pub end_time_ns: u64,
    pub algo_type: String,
    pub num_slices: usize,
    pub status: ParentOrderStatus,
    pub child_order_ids: Vec<OrderId>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub quantity: i64,
    pub avg_price: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
}
