use crate::risk::{RiskChecker, RiskConfig};
use crate::state::{ChildOrderInfo, EngineState};
use algo_core::twap::{compute_twap_schedule, TwapParams};
use api::{EngineCommand, ParentOrder, ParentOrderStatus};
use orderbook::{Fill, Order, OrderBook, Price, Quantity, Side, Timestamp};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration};
use tracing::{error, info};

/// Main execution engine that processes orders.
pub struct ExecutionEngine {
    state: EngineState,
    orderbooks: HashMap<String, OrderBook>,
    risk_checker: RiskChecker,
    tick_interval_ms: u64,
    fill_tx: broadcast::Sender<Fill>,
    /// Track which orderbooks have been seeded to prevent double-seeding
    seeded_orderbooks: HashMap<String, bool>,
}

impl ExecutionEngine {
    pub fn new(_symbol: String, tick_interval_ms: u64) -> Self {
        let (fill_tx, _) = broadcast::channel(1000);
        Self {
            state: EngineState::new(),
            orderbooks: HashMap::new(),
            risk_checker: RiskChecker::new(RiskConfig::default()),
            tick_interval_ms,
            fill_tx,
            seeded_orderbooks: HashMap::new(),
        }
    }

    /// Get or create an orderbook for a symbol
    fn get_or_create_orderbook(&mut self, symbol: &str) -> &mut OrderBook {
        self.orderbooks
            .entry(symbol.to_string())
            .or_insert_with(|| OrderBook::new(symbol.to_string()))
    }

    pub fn with_risk_config(mut self, config: RiskConfig) -> Self {
        self.risk_checker = RiskChecker::new(config);
        self
    }

    /// Run the main event loop.
    pub async fn run(mut self, mut cmd_rx: mpsc::Receiver<EngineCommand>) {
        info!(
            "Starting execution engine with {}ms tick interval",
            self.tick_interval_ms
        );

        let mut tick = interval(Duration::from_millis(self.tick_interval_ms));

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.process_clock_tick().await;
                }

                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        EngineCommand::SubmitParentOrder {
                            parent_id,
                            symbol,
                            side,
                            qty,
                            limit_price,
                            start_ns,
                            end_ns,
                            num_slices,
                        } => {
                            self.handle_submit_parent_order(
                                parent_id,
                                symbol,
                                side,
                                qty,
                                limit_price,
                                start_ns,
                                end_ns,
                                num_slices,
                            );
                        }

                        EngineCommand::QueryStatus { parent_id, response_tx } => {
                            let status = self.state.get_parent_order(parent_id).cloned();
                            let _ = response_tx.send(status);
                        }

                        EngineCommand::GetPositions { symbol, response_tx } => {
                            let positions = if let Some(sym) = symbol {
                                // Get current market price from orderbook
                                let current_price = self.orderbooks.get(&sym)
                                    .and_then(|book| {
                                        // Use mid price if available, otherwise use best bid/ask
                                        let bid = book.best_bid().map(|p| p.as_f64());
                                        let ask = book.best_ask().map(|p| p.as_f64());
                                        match (bid, ask) {
                                            (Some(b), Some(a)) => Some((b + a) / 2.0),
                                            (Some(b), None) => Some(b),
                                            (None, Some(a)) => Some(a),
                                            (None, None) => None,
                                        }
                                    });

                                if let Some(price) = current_price {
                                    self.state.get_position_with_pnl(&sym, price).into_iter().collect()
                                } else {
                                    // No market price available, return position without updated unrealized PnL
                                    self.state.get_position(&sym).cloned().into_iter().collect()
                                }
                            } else {
                                // Get all positions with calculated unrealized PnL
                                self.state.positions.values().map(|pos| {
                                    let current_price = self.orderbooks.get(&pos.symbol)
                                        .and_then(|book| {
                                            let bid = book.best_bid().map(|p| p.as_f64());
                                            let ask = book.best_ask().map(|p| p.as_f64());
                                            match (bid, ask) {
                                                (Some(b), Some(a)) => Some((b + a) / 2.0),
                                                (Some(b), None) => Some(b),
                                                (None, Some(a)) => Some(a),
                                                (None, None) => None,
                                            }
                                        });

                                    if let Some(price) = current_price {
                                        let mut position = pos.clone();
                                        position.unrealized_pnl = (price - position.avg_price) * position.quantity as f64;
                                        position
                                    } else {
                                        pos.clone()
                                    }
                                }).collect()
                            };
                            let _ = response_tx.send(positions);
                        }

                        EngineCommand::SubscribeToFills { response_tx } => {
                            let receiver = self.fill_tx.subscribe();
                            let _ = response_tx.send(receiver);
                        }

                        EngineCommand::Shutdown => {
                            info!("Shutdown command received");
                            break;
                        }
                    }
                }
            }
        }

        info!(
            "Engine shutdown complete. Total fills: {}, Final positions: {}",
            self.state.fills.len(),
            self.state.positions.len()
        );
    }

    /// Process scheduled child orders on each clock tick.
    async fn process_clock_tick(&mut self) {
        let now_ns = Timestamp::now_nanos().nanos();

        // Log tick processing for debugging
        if !self.state.pending_children.is_empty() {
            info!(
                "[TICK] now={} ns, pending_children={}, first_target={}",
                now_ns,
                self.state.pending_children.len(),
                self.state
                    .pending_children
                    .first()
                    .map(|(t, _)| *t)
                    .unwrap_or(0)
            );
        }

        // Process pending children whose target time has arrived
        let mut i = 0;
        while i < self.state.pending_children.len() {
            let (target_time_ns, _) = self.state.pending_children[i];

            if target_time_ns <= now_ns {
                let (_, order) = self.state.pending_children.remove(i);
                info!(
                    "[EXEC] Executing child order {} at target time {} (now={})",
                    order.id, target_time_ns, now_ns
                );

                // Get the symbol for this order from its parent
                let symbol = self
                    .state
                    .get_child_order(&order.id)
                    .and_then(|child_info| self.state.get_parent_order(child_info.parent_id))
                    .map(|parent| parent.symbol.clone())
                    .unwrap_or_else(|| "UNKNOWN".to_string());

                // Execute order against the correct orderbook for this symbol
                let orderbook = self.get_or_create_orderbook(&symbol);
                let fills = orderbook.insert_order(order.clone());

                // Process fills
                for fill in fills {
                    info!(
                        "Fill: order={} qty={} price={} time={}",
                        fill.order_id, fill.fill_qty, fill.fill_price, fill.timestamp
                    );

                    // Update position
                    self.state.update_position(&symbol, &fill, order.side);

                    // Update child order status
                    if let Some(child_info) = self.state.get_child_order_mut(&fill.order_id) {
                        child_info.filled_qty = child_info.filled_qty.saturating_add(fill.fill_qty);
                        child_info.status = "FILLED".to_string();
                    }

                    // Update parent order filled quantity
                    // Find parent ID from child order
                    if let Some(child_info) = self.state.get_child_order(&fill.order_id) {
                        if let Some(parent) = self.state.get_parent_order_mut(child_info.parent_id)
                        {
                            parent.filled_qty = parent.filled_qty.saturating_add(fill.fill_qty);

                            // Update parent status
                            if parent.filled_qty >= parent.total_qty {
                                parent.status = ParentOrderStatus::Filled;
                                info!("Parent order {} fully filled", parent.id);
                            } else if parent.filled_qty > Quantity::ZERO {
                                parent.status = ParentOrderStatus::PartiallyFilled;
                            }
                        }
                    }

                    let _ = self.fill_tx.send(fill.clone());
                    self.state.add_fill(fill);
                }

                // Don't increment i, we removed an element
            } else {
                i += 1;
            }
        }
    }

    /// Handle submission of a parent order.
    #[allow(clippy::too_many_arguments)]
    fn handle_submit_parent_order(
        &mut self,
        parent_id: u64,
        symbol: String,
        side: Side,
        qty: u64,
        limit_price: Option<Price>,
        start_ns: u64,
        end_ns: u64,
        num_slices: usize,
    ) {
        info!(
            "Received parent order {}: {} {} {} @ {:?}",
            parent_id, side, qty, symbol, limit_price
        );

        let parent_order = ParentOrder {
            id: parent_id,
            symbol: symbol.clone(),
            side,
            total_qty: Quantity::new(qty),
            filled_qty: Quantity::ZERO,
            limit_price,
            start_time_ns: start_ns,
            end_time_ns: end_ns,
            algo_type: "TWAP".to_string(),
            num_slices,
            status: ParentOrderStatus::Pending,
            child_order_ids: Vec::new(),
            created_at: Timestamp::now_nanos(),
        };

        // Risk checks
        if let Err(e) = self
            .risk_checker
            .check_parent_order(&parent_order, &self.state)
        {
            error!("Risk check failed for parent order {}: {}", parent_id, e);
            let mut rejected_order = parent_order;
            rejected_order.status = ParentOrderStatus::Rejected;
            self.state.add_parent_order(rejected_order);
            return;
        }

        // Compute TWAP schedule
        let schedule = compute_twap_schedule(TwapParams {
            start_ns,
            end_ns,
            total_qty: qty,
            num_slices,
        });

        info!(
            "Generated {} child orders for parent {}",
            schedule.len(),
            parent_id
        );

        // Create child orders
        let mut child_order_ids = Vec::new();

        // Check if we need to seed the orderbook (before getting mutable borrow)
        let needs_seeding = !self.seeded_orderbooks.contains_key(&symbol);

        // Get or create orderbook for this symbol
        let orderbook = self.get_or_create_orderbook(&symbol);

        // Seed orderbook with market maker liquidity if not already seeded
        if needs_seeding {
            let mid_price = Price::from_f64(100.0);
            info!(
                "Seeding orderbook {} with market maker liquidity at price {}",
                symbol, mid_price.0
            );
            orderbook.seed_market_maker(mid_price, 10, Quantity::new(10000));
        }

        // For market orders, use aggressive pricing to cross the spread
        let price = limit_price.unwrap_or_else(|| {
            match side {
                Side::Buy => {
                    // BUY orders: use best ask price (or higher to guarantee fill)
                    orderbook
                        .best_ask()
                        .map(|p| Price(p.0 + 100)) // Pay slightly above best ask
                        .unwrap_or(Price::from_f64(100.1))
                }
                Side::Sell => {
                    // SELL orders: use best bid price (or lower to guarantee fill)
                    orderbook
                        .best_bid()
                        .map(|p| Price(p.0 - 100)) // Sell slightly below best bid
                        .unwrap_or(Price::from_f64(99.9))
                }
            }
        });

        // Mark orderbook as seeded (must be done after we're finished with orderbook)
        if needs_seeding {
            self.seeded_orderbooks.insert(symbol.clone(), true);
        }

        for instr in schedule {
            let child_id = self.state.next_child_id();
            let order = Order::new(
                child_id,
                side,
                price,
                Quantity::new(instr.qty),
                Timestamp::new(instr.target_time_ns),
            );

            // Track child order
            self.state.add_child_order(ChildOrderInfo {
                id: child_id,
                parent_id,
                status: "PENDING".to_string(),
                filled_qty: Quantity::ZERO,
                target_time_ns: instr.target_time_ns,
            });

            self.state
                .pending_children
                .push((instr.target_time_ns, order));
            child_order_ids.push(child_id);
        }

        // Store parent order
        let mut parent = parent_order;
        parent.child_order_ids = child_order_ids;
        parent.status = ParentOrderStatus::Working;
        self.state.add_parent_order(parent);

        info!("Parent order {} accepted and working", parent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_engine_startup_shutdown() {
        let (tx, rx) = mpsc::channel(32);
        let engine = ExecutionEngine::new("BTC-USD".to_string(), 100);

        let handle = tokio::spawn(async move {
            engine.run(rx).await;
        });

        // Send shutdown command
        tx.send(EngineCommand::Shutdown).await.unwrap();

        // Wait for engine to shutdown
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_submit_parent_order() {
        let (tx, rx) = mpsc::channel(32);
        let engine = ExecutionEngine::new("BTC-USD".to_string(), 100);

        let handle = tokio::spawn(async move {
            engine.run(rx).await;
        });

        // Submit a parent order
        tx.send(EngineCommand::SubmitParentOrder {
            parent_id: 1,
            symbol: "BTC-USD".to_string(),
            side: Side::Buy,
            qty: 100,
            limit_price: Some(Price::from_f64(50000.0)),
            start_ns: Timestamp::now_nanos().nanos(),
            end_ns: Timestamp::now_nanos().nanos() + 5_000_000_000, // 5 seconds from now
            num_slices: 5,
        })
        .await
        .unwrap();

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Query status
        let (status_tx, status_rx) = tokio::sync::oneshot::channel();
        tx.send(EngineCommand::QueryStatus {
            parent_id: 1,
            response_tx: status_tx,
        })
        .await
        .unwrap();

        let status = status_rx.await.unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap().id, 1);

        // Shutdown
        tx.send(EngineCommand::Shutdown).await.unwrap();
        handle.await.unwrap();
    }
}
