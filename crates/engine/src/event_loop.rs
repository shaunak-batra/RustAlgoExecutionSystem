//! Async task that owns an [`EngineCore`] and serves [`EngineCommand`]s.

use crate::executor::EngineCore;
use api::{EngineCommand, FillReport};
use orderbook::Timestamp;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, MissedTickBehavior};
use tracing::info;

/// The engine task.
///
/// One task owns all engine state. The gRPC layer reaches it only through the
/// command channel, so there is no shared mutable state and no locking. Timer
/// ticks release due child orders, and fills are broadcast to subscribers.
pub struct ExecutionEngine {
    core: EngineCore,
    tick_interval: Duration,
    fills: broadcast::Sender<FillReport>,
}

impl ExecutionEngine {
    /// `fill_buffer` is roughly how many fills a slow subscriber may fall behind
    /// before its stream ends with DATA_LOSS: tokio rounds the broadcast capacity
    /// up to a power of two, and the gRPC layer buffers up to 256 more fills per
    /// stream on top of that.
    pub fn new(core: EngineCore, tick_interval: Duration, fill_buffer: usize) -> Self {
        let (fills, _) = broadcast::channel(fill_buffer.max(1));
        Self {
            core,
            tick_interval,
            fills,
        }
    }

    /// Runs until a `Shutdown` command arrives or every command sender is dropped.
    pub async fn run(mut self, mut commands: mpsc::Receiver<EngineCommand>) {
        let mut ticker = interval(self.tick_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        info!("engine started with a {:?} tick", self.tick_interval);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    for fill in self.core.tick(Timestamp::now_nanos().nanos()) {
                        // An error only means nobody is subscribed right now.
                        let _ = self.fills.send(fill);
                    }
                }
                command = commands.recv() => match command {
                    Some(EngineCommand::Shutdown) | None => break,
                    Some(command) => self.handle(command),
                },
            }
        }

        info!("engine stopped");
    }

    fn handle(&mut self, command: EngineCommand) {
        let now_ns = Timestamp::now_nanos().nanos();
        // A failed reply only means the requester went away.
        match command {
            EngineCommand::SubmitParentOrder { order, reply } => {
                let _ = reply.send(self.core.submit(order, now_ns).map_err(|e| e.to_string()));
            }
            EngineCommand::CancelParentOrder {
                parent_order_id,
                reply,
            } => {
                let _ = reply.send(self.core.cancel(parent_order_id).map_err(|e| e.to_string()));
            }
            EngineCommand::GetOrderStatus {
                parent_order_id,
                reply,
            } => {
                let _ = reply.send(self.core.order_status(parent_order_id));
            }
            EngineCommand::GetPositions { symbol, reply } => {
                let _ = reply.send(self.core.positions(symbol.as_deref()));
            }
            EngineCommand::SubscribeFills { reply } => {
                let _ = reply.send(self.fills.subscribe());
            }
            EngineCommand::Shutdown => {}
        }
    }
}
