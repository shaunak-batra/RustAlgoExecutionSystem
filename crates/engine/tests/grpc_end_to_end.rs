//! End-to-end test: the engine task, the gRPC server and a tonic client over a
//! real loopback socket.

use api::proto::execution_service_client::ExecutionServiceClient;
use api::proto::submit_parent_order_request::Algorithm;
use api::proto::{
    CancelParentOrderRequest, OrderStatusRequest, ParentOrderState, PositionsRequest, Side,
    StreamFillsRequest, SubmitParentOrderRequest, TwapParams,
};
use api::EngineCommand;
use engine::{EngineConfig, EngineCore, ExecutionEngine, RiskLimits, ServiceOptions, SymbolConfig};
use orderbook::{Price, Timestamp};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

fn core() -> EngineCore {
    EngineCore::new(EngineConfig {
        risk: RiskLimits {
            max_order_qty: 1_000_000,
            max_order_notional: i128::MAX,
            max_position_qty: 1_000_000,
            max_gross_notional: i128::MAX,
            price_collar_bps: 10_000,
            max_loss: i128::MAX,
        },
        symbols: vec![SymbolConfig {
            symbol: "SIM".to_string(),
            reference_price: Price::from_f64(100.0),
            levels: 5,
            qty_per_level: 1_000,
            level_spacing_bps: 1,
        }],
        retained_finished_orders: 100,
    })
    .unwrap()
}

/// A TWAP over the next second in four slices.
fn twap_request(symbol: &str, side: Side, quantity: u64) -> SubmitParentOrderRequest {
    let now = Timestamp::now_nanos().nanos();
    SubmitParentOrderRequest {
        symbol: symbol.to_string(),
        side: side as i32,
        quantity,
        limit_price_ticks: None,
        start_time_ns: now,
        end_time_ns: now + 1_000_000_000,
        algorithm: Some(Algorithm::Twap(TwapParams { num_slices: 4 })),
    }
}

#[tokio::test]
async fn orders_fills_status_and_positions_over_grpc() {
    let (commands, receiver) = mpsc::channel(64);
    let engine = ExecutionEngine::new(core(), Duration::from_millis(5), 1_024);
    let engine_task = tokio::spawn(engine.run(receiver));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_server, server_stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(api::serve_with_listener(
        commands.clone(),
        listener,
        async {
            let _ = server_stopped.await;
        },
    ));

    let mut client = ExecutionServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let mut fills = client
        .stream_fills(StreamFillsRequest {})
        .await
        .unwrap()
        .into_inner();

    let submitted = client
        .submit_parent_order(twap_request("SIM", Side::Buy, 100))
        .await
        .unwrap()
        .into_inner();
    assert!(submitted.accepted, "rejected: {}", submitted.reject_reason);

    let mut filled = 0;
    while filled < 100 {
        let event = timeout(WAIT, fills.message())
            .await
            .expect("fills arrive")
            .unwrap()
            .expect("the stream stays open");
        assert_eq!(event.parent_order_id, submitted.parent_order_id);
        assert_eq!(event.symbol, "SIM");
        assert_eq!(event.side, Side::Buy as i32);
        filled += event.quantity;
    }
    assert_eq!(filled, 100);

    let status = timeout(WAIT, async {
        loop {
            let status = client
                .get_order_status(OrderStatusRequest {
                    parent_order_id: submitted.parent_order_id,
                })
                .await
                .unwrap()
                .into_inner();
            if status.state != ParentOrderState::Working as i32 {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the order finishes");
    assert_eq!(status.state, ParentOrderState::Filled as i32);
    assert_eq!(status.filled_quantity, 100);
    assert_eq!(status.children.len(), 4);
    assert!(status.average_fill_price_ticks.is_some());

    let positions = client
        .get_positions(PositionsRequest {
            symbol: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(positions.positions.len(), 1);
    assert_eq!(positions.positions[0].quantity, 100);
    assert!(!positions.trading_halted);

    // Business rejections come back in the response...
    let unknown = client
        .submit_parent_order(twap_request("NOPE", Side::Buy, 10))
        .await
        .unwrap()
        .into_inner();
    assert!(!unknown.accepted);
    assert!(
        unknown.reject_reason.contains("unknown symbol"),
        "{}",
        unknown.reject_reason
    );
    let cancel = client
        .cancel_parent_order(CancelParentOrderRequest {
            parent_order_id: 999,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!cancel.cancelled);

    // ...while malformed requests, and unknown ids on GetOrderStatus, are gRPC errors.
    let unspecified_side = client
        .submit_parent_order(twap_request("SIM", Side::Unspecified, 10))
        .await
        .unwrap_err();
    assert_eq!(unspecified_side.code(), tonic::Code::InvalidArgument);
    let missing = client
        .get_order_status(OrderStatusRequest {
            parent_order_id: 999,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound);

    drop(fills);
    drop(client);
    commands.send(EngineCommand::Shutdown).await.unwrap();
    engine_task.await.unwrap();
    stop_server.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_finishes_while_a_fill_stream_is_open() {
    // The server waits for open streams to end, and a fill stream only ends when
    // the engine stops. engine::serve stops the engine as part of the shutdown
    // signal; the binary used to stop it afterwards, which hung here forever.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let options = ServiceOptions {
        tick_interval: Duration::from_millis(5),
        command_buffer: 64,
        fill_buffer: 1_024,
    };
    let service = tokio::spawn(engine::serve(core(), options, listener, async {
        let _ = stopped.await;
    }));

    let mut client = ExecutionServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let mut fills = client
        .stream_fills(StreamFillsRequest {})
        .await
        .unwrap()
        .into_inner();

    stop.send(()).unwrap();
    timeout(WAIT, service)
        .await
        .expect("shutdown must not hang while a fill stream is open")
        .unwrap()
        .unwrap();
    let last = timeout(WAIT, fills.message())
        .await
        .expect("the stream ends");
    assert!(
        !matches!(last, Ok(Some(_))),
        "no fill was traded, so the stream can only have ended"
    );
}
