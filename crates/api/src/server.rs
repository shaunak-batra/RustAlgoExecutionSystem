use crate::proto::execution_service_server::{ExecutionService, ExecutionServiceServer};
use crate::proto::*;
use crate::types::{EngineCommand, ParentOrderStatus};
use orderbook::{Price, Side};
use std::str::FromStr;
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status};

/// gRPC service implementation for the execution engine.
pub struct ExecutionServiceImpl {
    engine_tx: mpsc::Sender<EngineCommand>,
}

impl ExecutionServiceImpl {
    pub fn new(engine_tx: mpsc::Sender<EngineCommand>) -> Self {
        Self { engine_tx }
    }
}

#[tonic::async_trait]
impl ExecutionService for ExecutionServiceImpl {
    async fn submit_parent_order(
        &self,
        request: Request<ParentOrderRequest>,
    ) -> Result<Response<ParentOrderResponse>, Status> {
        let req = request.into_inner();

        // Generate parent order ID
        let parent_id = rand::random::<u64>();

        // Parse side
        let side = Side::from_str(&req.side)
            .map_err(|e| Status::invalid_argument(format!("Invalid side: {}", e)))?;

        // Parse limit price
        let limit_price = if req.limit_price_ticks != 0 {
            Some(Price::new(req.limit_price_ticks))
        } else {
            None
        };

        // Send command to engine
        self.engine_tx
            .send(EngineCommand::SubmitParentOrder {
                parent_id,
                symbol: req.symbol,
                side,
                qty: req.quantity,
                limit_price,
                start_ns: req.start_time_ns,
                end_ns: req.end_time_ns,
                num_slices: req.num_slices as usize,
            })
            .await
            .map_err(|_| Status::internal("Engine unavailable"))?;

        Ok(Response::new(ParentOrderResponse {
            parent_order_id: parent_id,
            status: "ACCEPTED".to_string(),
            message: String::new(),
        }))
    }

    async fn get_order_status(
        &self,
        request: Request<OrderStatusRequest>,
    ) -> Result<Response<OrderStatusResponse>, Status> {
        let req = request.into_inner();

        // Create oneshot channel for response
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Send query command
        self.engine_tx
            .send(EngineCommand::QueryStatus {
                parent_id: req.parent_order_id,
                response_tx: tx,
            })
            .await
            .map_err(|_| Status::internal("Engine unavailable"))?;

        // Wait for response
        let parent_order = rx
            .await
            .map_err(|_| Status::internal("Failed to receive response"))?
            .ok_or_else(|| Status::not_found("Order not found"))?;

        // Convert to response
        let status = match parent_order.status {
            ParentOrderStatus::Pending => "PENDING",
            ParentOrderStatus::Working => "WORKING",
            ParentOrderStatus::Filled => "FILLED",
            ParentOrderStatus::PartiallyFilled => "PARTIALLY_FILLED",
            ParentOrderStatus::Cancelled => "CANCELLED",
            ParentOrderStatus::Rejected => "REJECTED",
        }
        .to_string();

        let avg_fill_price = if parent_order.filled_qty.value() > 0 {
            parent_order.limit_price.map(|p| p.as_f64()).unwrap_or(0.0)
        } else {
            0.0
        };

        Ok(Response::new(OrderStatusResponse {
            parent_order_id: parent_order.id,
            status,
            filled_qty: parent_order.filled_qty.value(),
            avg_fill_price,
            children: vec![],
        }))
    }

    async fn get_positions(
        &self,
        request: Request<PositionsRequest>,
    ) -> Result<Response<PositionsResponse>, Status> {
        let req = request.into_inner();

        // Create oneshot channel
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Query positions
        self.engine_tx
            .send(EngineCommand::GetPositions {
                symbol: if req.symbol.is_empty() {
                    None
                } else {
                    Some(req.symbol.clone())
                },
                response_tx: tx,
            })
            .await
            .map_err(|_| Status::internal("Engine unavailable"))?;

        let positions = rx
            .await
            .map_err(|_| Status::internal("Failed to receive response"))?;

        if let Some(pos) = positions.first() {
            Ok(Response::new(PositionsResponse {
                symbol: pos.symbol.clone(),
                position: pos.quantity,
                realized_pnl: pos.realized_pnl,
                unrealized_pnl: pos.unrealized_pnl,
            }))
        } else {
            Ok(Response::new(PositionsResponse {
                symbol: req.symbol,
                position: 0,
                realized_pnl: 0.0,
                unrealized_pnl: 0.0,
            }))
        }
    }

    type StreamFillsStream = tokio_stream::wrappers::ReceiverStream<Result<FillEvent, Status>>;

    async fn stream_fills(
        &self,
        _request: Request<StreamFillsRequest>,
    ) -> Result<Response<Self::StreamFillsStream>, Status> {
        let (subscribe_tx, subscribe_rx) = tokio::sync::oneshot::channel();

        self.engine_tx
            .send(EngineCommand::SubscribeToFills {
                response_tx: subscribe_tx,
            })
            .await
            .map_err(|_| Status::internal("Engine unavailable"))?;

        let mut fill_rx = subscribe_rx
            .await
            .map_err(|_| Status::internal("Failed to subscribe to fills"))?;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        // Spawn a task to forward fill events to the client
        tokio::spawn(async move {
            loop {
                match fill_rx.recv().await {
                    Ok(fill) => {
                        let fill_event = FillEvent {
                            order_id: fill.taker_order_id.value(),
                            fill_qty: fill.qty.value(),
                            fill_price_ticks: fill.price.ticks(),
                            timestamp_ns: fill.timestamp.nanos(),
                        };

                        if tx.send(Ok(fill_event)).await.is_err() {
                            // Client disconnected, gracefully exit
                            println!("Fill stream closed: client disconnected");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        eprintln!("Fill stream lagged, skipped {} messages", skipped);
                        // Notify client about the lag
                        let error_msg = format!("Fill stream lagged: {} messages skipped", skipped);
                        if tx.send(Err(Status::data_loss(error_msg))).await.is_err() {
                            // Client disconnected while sending error, gracefully exit
                            println!(
                                "Fill stream closed: client disconnected during error notification"
                            );
                            break;
                        }
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Broadcast channel closed, gracefully exit
                        println!("Fill stream closed: engine shut down");
                        break;
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }
}

/// Start the gRPC server.
pub async fn start_grpc_server(
    engine_tx: mpsc::Sender<EngineCommand>,
    addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = addr.parse()?;
    let service = ExecutionServiceImpl::new(engine_tx);

    println!("gRPC server listening on {}", addr);

    Server::builder()
        .add_service(ExecutionServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
