//! gRPC front end: translates protobuf messages into engine commands and back.

use crate::proto::execution_service_server::{ExecutionService, ExecutionServiceServer};
use crate::proto::{self, submit_parent_order_request::Algorithm};
use crate::types::{
    AlgorithmSpec, ChildOrderView, EngineCommand, FillReport, NewParentOrder, ParentOrderState,
    ParentOrderView, PositionView,
};
use orderbook::{Price, Side};
use std::future::Future;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{transport::Server, Request, Response, Status};

/// gRPC service that forwards every request to the engine task.
pub struct ExecutionServiceImpl {
    engine: mpsc::Sender<EngineCommand>,
}

impl ExecutionServiceImpl {
    pub fn new(engine: mpsc::Sender<EngineCommand>) -> Self {
        Self { engine }
    }

    /// Sends a command carrying a fresh reply channel and waits for the answer.
    async fn ask<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<T>) -> EngineCommand,
    ) -> Result<T, Status> {
        let (reply, answer) = oneshot::channel();
        self.engine
            .send(command(reply))
            .await
            .map_err(|_| Status::unavailable("the engine is not running"))?;
        answer
            .await
            .map_err(|_| Status::unavailable("the engine stopped before replying"))
    }
}

#[tonic::async_trait]
impl ExecutionService for ExecutionServiceImpl {
    async fn submit_parent_order(
        &self,
        request: Request<proto::SubmitParentOrderRequest>,
    ) -> Result<Response<proto::SubmitParentOrderResponse>, Status> {
        let order = new_parent_order(request.into_inner()).map_err(Status::invalid_argument)?;
        let outcome = self
            .ask(move |reply| EngineCommand::SubmitParentOrder { order, reply })
            .await?;
        let response = match outcome {
            Ok(parent_order_id) => proto::SubmitParentOrderResponse {
                accepted: true,
                parent_order_id,
                reject_reason: String::new(),
            },
            Err(reject_reason) => proto::SubmitParentOrderResponse {
                accepted: false,
                parent_order_id: 0,
                reject_reason,
            },
        };
        Ok(Response::new(response))
    }

    async fn cancel_parent_order(
        &self,
        request: Request<proto::CancelParentOrderRequest>,
    ) -> Result<Response<proto::CancelParentOrderResponse>, Status> {
        let parent_order_id = request.into_inner().parent_order_id;
        let outcome = self
            .ask(|reply| EngineCommand::CancelParentOrder {
                parent_order_id,
                reply,
            })
            .await?;
        let response = match outcome {
            Ok(()) => proto::CancelParentOrderResponse {
                cancelled: true,
                reason: String::new(),
            },
            Err(reason) => proto::CancelParentOrderResponse {
                cancelled: false,
                reason,
            },
        };
        Ok(Response::new(response))
    }

    async fn get_order_status(
        &self,
        request: Request<proto::OrderStatusRequest>,
    ) -> Result<Response<proto::OrderStatusResponse>, Status> {
        let parent_order_id = request.into_inner().parent_order_id;
        let view = self
            .ask(|reply| EngineCommand::GetOrderStatus {
                parent_order_id,
                reply,
            })
            .await?
            .ok_or_else(|| Status::not_found(format!("no parent order {parent_order_id}")))?;
        Ok(Response::new(order_status(view)))
    }

    async fn get_positions(
        &self,
        request: Request<proto::PositionsRequest>,
    ) -> Result<Response<proto::PositionsResponse>, Status> {
        let symbol = request.into_inner().symbol;
        let symbol = (!symbol.is_empty()).then_some(symbol);
        let snapshot = self
            .ask(|reply| EngineCommand::GetPositions { symbol, reply })
            .await?;
        Ok(Response::new(proto::PositionsResponse {
            positions: snapshot
                .positions
                .into_iter()
                .map(position_report)
                .collect(),
            trading_halted: snapshot.halt_reason.is_some(),
            halt_reason: snapshot.halt_reason.unwrap_or_default(),
        }))
    }

    type StreamFillsStream = ReceiverStream<Result<proto::FillEvent, Status>>;

    async fn stream_fills(
        &self,
        _request: Request<proto::StreamFillsRequest>,
    ) -> Result<Response<Self::StreamFillsStream>, Status> {
        let mut fills = self
            .ask(|reply| EngineCommand::SubscribeFills { reply })
            .await?;
        let (sender, receiver) = mpsc::channel(256);

        tokio::spawn(async move {
            loop {
                let item = match fills.recv().await {
                    Ok(fill) => Ok(fill_event(fill)),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        Err(Status::data_loss(format!(
                            "the fill stream fell behind and skipped {skipped} fills; \
                             resubscribe and reconcile with GetOrderStatus"
                        )))
                    }
                    // The engine has shut down.
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let ends_stream = item.is_err();
                // A send error means the client went away.
                if sender.send(item).await.is_err() || ends_stream {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

/// Serves the API on `addr` until `shutdown` completes.
pub async fn serve(
    engine: mpsc::Sender<EngineCommand>,
    addr: SocketAddr,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(ExecutionServiceServer::new(ExecutionServiceImpl::new(
            engine,
        )))
        .serve_with_shutdown(addr, shutdown)
        .await
}

/// Serves the API on an already bound listener until `shutdown` completes.
/// Binding to port 0 first lets tests use any free port.
pub async fn serve_with_listener(
    engine: mpsc::Sender<EngineCommand>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(ExecutionServiceServer::new(ExecutionServiceImpl::new(
            engine,
        )))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown)
        .await
}

/// Converts a request into an engine order. The error is the reason the request
/// is malformed, which the caller reports as `INVALID_ARGUMENT`.
fn new_parent_order(
    request: proto::SubmitParentOrderRequest,
) -> Result<NewParentOrder, &'static str> {
    let side = match proto::Side::try_from(request.side) {
        Ok(proto::Side::Buy) => Side::Buy,
        Ok(proto::Side::Sell) => Side::Sell,
        _ => return Err("side must be SIDE_BUY or SIDE_SELL"),
    };
    let algorithm = match request.algorithm {
        Some(Algorithm::Twap(params)) => AlgorithmSpec::Twap {
            num_slices: params.num_slices as usize,
        },
        Some(Algorithm::Vwap(params)) => AlgorithmSpec::Vwap {
            volume_profile: params.volume_profile,
        },
        Some(Algorithm::ImplementationShortfall(params)) => {
            AlgorithmSpec::ImplementationShortfall {
                num_slices: params.num_slices as usize,
                risk_aversion: params.risk_aversion,
                volatility: params.volatility,
                temporary_impact: params.temporary_impact,
                permanent_impact: params.permanent_impact,
            }
        }
        None => return Err("an algorithm must be set"),
    };
    Ok(NewParentOrder {
        symbol: request.symbol,
        side,
        quantity: request.quantity,
        limit_price: request.limit_price_ticks.map(Price::new),
        start_ns: request.start_time_ns,
        end_ns: request.end_time_ns,
        algorithm,
    })
}

fn side_to_proto(side: Side) -> i32 {
    (match side {
        Side::Buy => proto::Side::Buy,
        Side::Sell => proto::Side::Sell,
    }) as i32
}

fn state_to_proto(state: ParentOrderState) -> i32 {
    (match state {
        ParentOrderState::Working => proto::ParentOrderState::Working,
        ParentOrderState::Filled => proto::ParentOrderState::Filled,
        ParentOrderState::Cancelled => proto::ParentOrderState::Cancelled,
        ParentOrderState::Expired => proto::ParentOrderState::Expired,
    }) as i32
}

/// Clamps an exact 128-bit money amount into the int64 range used on the wire.
fn saturate(value: i128) -> i64 {
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn order_status(view: ParentOrderView) -> proto::OrderStatusResponse {
    proto::OrderStatusResponse {
        parent_order_id: view.id,
        symbol: view.symbol,
        side: side_to_proto(view.side),
        algorithm: view.algorithm.to_string(),
        state: state_to_proto(view.state),
        state_reason: view.state_reason.unwrap_or_default(),
        quantity: view.quantity,
        filled_quantity: view.filled_quantity,
        limit_price_ticks: view.limit_price.map(|price| price.ticks()),
        start_time_ns: view.start_ns,
        end_time_ns: view.end_ns,
        arrival_mid_ticks: view.arrival_mid.ticks(),
        average_fill_price_ticks: view.average_fill_price_ticks,
        shortfall_bps: view.shortfall_bps,
        pending_slices: u32::try_from(view.pending_slices).unwrap_or(u32::MAX),
        children: view.children.into_iter().map(child_report).collect(),
    }
}

fn child_report(child: ChildOrderView) -> proto::ChildOrderReport {
    proto::ChildOrderReport {
        child_order_id: child.child_order_id,
        sent_at_ns: child.sent_at_ns,
        quantity: child.quantity,
        filled_quantity: child.filled_quantity,
    }
}

fn position_report(position: PositionView) -> proto::PositionReport {
    proto::PositionReport {
        symbol: position.symbol,
        quantity: position.quantity,
        average_price_ticks: position.average_price_ticks,
        realized_pnl: saturate(position.realized_pnl),
        unrealized_pnl: position.unrealized_pnl.map(saturate),
        mark_price_ticks: position.mark_price.map(|price| price.ticks()),
    }
}

fn fill_event(fill: FillReport) -> proto::FillEvent {
    proto::FillEvent {
        parent_order_id: fill.parent_order_id,
        child_order_id: fill.child_order_id,
        symbol: fill.symbol,
        side: side_to_proto(fill.side),
        price_ticks: fill.price.ticks(),
        quantity: fill.quantity,
        timestamp_ns: fill.timestamp_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> proto::SubmitParentOrderRequest {
        proto::SubmitParentOrderRequest {
            symbol: "SIM".to_string(),
            side: proto::Side::Buy as i32,
            quantity: 10,
            limit_price_ticks: Some(12_345),
            start_time_ns: 1,
            end_time_ns: 2,
            algorithm: Some(Algorithm::Twap(proto::TwapParams { num_slices: 4 })),
        }
    }

    #[test]
    fn converts_a_well_formed_request() {
        assert_eq!(
            new_parent_order(request()).unwrap(),
            NewParentOrder {
                symbol: "SIM".to_string(),
                side: Side::Buy,
                quantity: 10,
                limit_price: Some(Price::new(12_345)),
                start_ns: 1,
                end_ns: 2,
                algorithm: AlgorithmSpec::Twap { num_slices: 4 },
            }
        );
    }

    #[test]
    fn unset_limit_price_means_market_orders() {
        let order = new_parent_order(proto::SubmitParentOrderRequest {
            limit_price_ticks: None,
            ..request()
        })
        .unwrap();
        assert_eq!(order.limit_price, None);
    }

    #[test]
    fn rejects_an_unspecified_or_unknown_side() {
        for side in [proto::Side::Unspecified as i32, 99] {
            let reason = new_parent_order(proto::SubmitParentOrderRequest { side, ..request() })
                .unwrap_err();
            assert!(reason.contains("side"), "{reason}");
        }
    }

    #[test]
    fn rejects_a_missing_algorithm() {
        let reason = new_parent_order(proto::SubmitParentOrderRequest {
            algorithm: None,
            ..request()
        })
        .unwrap_err();
        assert!(reason.contains("algorithm"), "{reason}");
    }

    #[test]
    fn money_saturates_to_the_int64_range() {
        assert_eq!(saturate(i128::MAX), i64::MAX);
        assert_eq!(saturate(i128::MIN), i64::MIN);
        assert_eq!(saturate(-42), -42);
    }
}
