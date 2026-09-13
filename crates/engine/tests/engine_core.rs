//! Scenario tests for the deterministic engine core. Time is passed in
//! explicitly, so every assertion is exact and every run is reproducible.

use api::{AlgorithmSpec, FillReport, NewParentOrder, ParentOrderState};
use engine::{
    CancelError, EngineConfig, EngineCore, RejectReason, RiskLimits, RiskViolation, SymbolConfig,
};
use orderbook::{Price, Side};

const SECOND: u64 = 1_000_000_000;
/// Reference mid of the simulated market: 100.00000.
const MID: i64 = 10_000_000;
/// One basis point of `MID`: the gap between quote levels.
const STEP: i64 = 1_000;

fn generous_limits() -> RiskLimits {
    RiskLimits {
        max_order_qty: 1_000_000,
        max_order_notional: i128::MAX,
        max_position_qty: 1_000_000,
        max_gross_notional: i128::MAX,
        price_collar_bps: 10_000,
        max_loss: i128::MAX,
    }
}

fn config(risk: RiskLimits, levels: u32, qty_per_level: u64, retained: usize) -> EngineConfig {
    EngineConfig {
        risk,
        symbols: vec![SymbolConfig {
            symbol: "SIM".to_string(),
            reference_price: Price::new(MID),
            levels,
            qty_per_level,
            level_spacing_bps: 1,
        }],
        retained_finished_orders: retained,
    }
}

fn engine_with(risk: RiskLimits, levels: u32, qty_per_level: u64) -> EngineCore {
    EngineCore::new(config(risk, levels, qty_per_level, 1_000)).unwrap()
}

fn engine(levels: u32, qty_per_level: u64) -> EngineCore {
    engine_with(generous_limits(), levels, qty_per_level)
}

fn order(
    side: Side,
    quantity: u64,
    start_s: u64,
    end_s: u64,
    algorithm: AlgorithmSpec,
) -> NewParentOrder {
    NewParentOrder {
        symbol: "SIM".to_string(),
        side,
        quantity,
        limit_price: None,
        start_ns: start_s * SECOND,
        end_ns: end_s * SECOND,
        algorithm,
    }
}

fn twap(side: Side, quantity: u64, start_s: u64, end_s: u64, num_slices: usize) -> NewParentOrder {
    order(
        side,
        quantity,
        start_s,
        end_s,
        AlgorithmSpec::Twap { num_slices },
    )
}

/// Ticks once per second from `from_s` to `to_s` inclusive and collects the fills.
fn run_seconds(engine: &mut EngineCore, from_s: u64, to_s: u64) -> Vec<FillReport> {
    (from_s..=to_s)
        .flat_map(|second| engine.tick(second * SECOND))
        .collect()
}

fn total_filled(fills: &[FillReport]) -> u64 {
    fills.iter().map(|fill| fill.quantity).sum()
}

fn prices_and_quantities(fills: &[FillReport]) -> Vec<(i64, u64)> {
    fills
        .iter()
        .map(|fill| (fill.price.ticks(), fill.quantity))
        .collect()
}

#[test]
fn twap_releases_one_child_per_slice_and_fills_completely() {
    let mut engine = engine(5, 1_000);
    let id = engine.submit(twap(Side::Buy, 1_000, 1, 11, 10), 0).unwrap();

    let fills = run_seconds(&mut engine, 0, 12);
    assert_eq!(fills.len(), 10);
    assert!(fills
        .iter()
        .all(|fill| fill.quantity == 100 && fill.price == Price::new(MID + STEP)));
    let times: Vec<u64> = fills.iter().map(|fill| fill.timestamp_ns).collect();
    assert_eq!(times, (1..=10).map(|s| s * SECOND).collect::<Vec<_>>());

    let status = engine.order_status(id).unwrap();
    assert_eq!(status.state, ParentOrderState::Filled);
    assert_eq!(status.filled_quantity, 1_000);
    assert_eq!(status.children.len(), 10);
    assert_eq!(status.pending_slices, 0);
    assert_eq!(status.average_fill_price_ticks, Some((MID + STEP) as f64));
    assert!((status.shortfall_bps.unwrap() - 1.0).abs() < 1e-9);

    let position = &engine.positions(None).positions[0];
    assert_eq!(position.quantity, 1_000);
    assert_eq!(position.unrealized_pnl, Some(-1_000 * i128::from(STEP)));
}

#[test]
fn unfilled_quantity_carries_into_the_next_child_and_the_order_expires() {
    // One level of 60 units per side: each child can fill at most 60.
    let mut engine = engine(1, 60);
    let id = engine.submit(twap(Side::Buy, 300, 1, 4, 3), 0).unwrap();

    let fills = run_seconds(&mut engine, 1, 4);
    assert_eq!(total_filled(&fills), 180);

    let status = engine.order_status(id).unwrap();
    let children: Vec<(u64, u64)> = status
        .children
        .iter()
        .map(|child| (child.quantity, child.filled_quantity))
        .collect();
    assert_eq!(children, vec![(100, 60), (140, 60), (180, 60)]);
    assert_eq!(status.state, ParentOrderState::Expired);
    assert_eq!(
        status.state_reason.as_deref(),
        Some("window ended with 120 of 300 unfilled")
    );
}

#[test]
fn limit_price_caps_every_child() {
    let mut engine = engine(3, 100);
    let mut capped = twap(Side::Buy, 250, 1, 2, 1);
    capped.limit_price = Some(Price::new(MID + 2 * STEP));
    let capped_id = engine.submit(capped, 0).unwrap();

    let fills = run_seconds(&mut engine, 1, 2);
    assert_eq!(
        prices_and_quantities(&fills),
        vec![(MID + STEP, 100), (MID + 2 * STEP, 100)]
    );
    let status = engine.order_status(capped_id).unwrap();
    assert_eq!(status.state, ParentOrderState::Expired);
    assert_eq!(status.filled_quantity, 200);

    let mut below_the_ask = twap(Side::Buy, 10, 3, 4, 1);
    below_the_ask.limit_price = Some(Price::new(MID));
    let below_id = engine.submit(below_the_ask, 2 * SECOND).unwrap();
    assert!(run_seconds(&mut engine, 3, 4).is_empty());
    assert_eq!(engine.order_status(below_id).unwrap().filled_quantity, 0);
}

#[test]
fn market_child_orders_walk_the_book() {
    let mut engine = engine(3, 100);
    let id = engine.submit(twap(Side::Buy, 250, 1, 2, 1), 0).unwrap();

    let fills = run_seconds(&mut engine, 1, 1);
    assert_eq!(
        prices_and_quantities(&fills),
        vec![
            (MID + STEP, 100),
            (MID + 2 * STEP, 100),
            (MID + 3 * STEP, 50)
        ]
    );
    let status = engine.order_status(id).unwrap();
    assert_eq!(status.state, ParentOrderState::Filled);
    assert_eq!(status.average_fill_price_ticks, Some((MID + 1_800) as f64));
    assert!((status.shortfall_bps.unwrap() - 1.8).abs() < 1e-9);
}

#[test]
fn slicing_a_large_order_pays_less_than_trading_it_at_once() {
    let average_price = |num_slices: usize| {
        let mut engine = engine(5, 100);
        let id = engine
            .submit(twap(Side::Buy, 500, 1, 6, num_slices), 0)
            .unwrap();
        run_seconds(&mut engine, 1, 6);
        let status = engine.order_status(id).unwrap();
        assert_eq!(status.filled_quantity, 500);
        status.average_fill_price_ticks.unwrap()
    };
    // One child sweeps all five levels; five children each take the refreshed best level.
    assert_eq!(average_price(1), (MID + 3 * STEP) as f64);
    assert_eq!(average_price(5), (MID + STEP) as f64);
}

#[test]
fn sell_orders_mirror_buys() {
    let mut engine = engine(5, 1_000);
    let id = engine.submit(twap(Side::Sell, 300, 1, 4, 3), 0).unwrap();

    let fills = run_seconds(&mut engine, 1, 4);
    assert!(fills
        .iter()
        .all(|fill| fill.side == Side::Sell && fill.price == Price::new(MID - STEP)));
    let status = engine.order_status(id).unwrap();
    assert_eq!(status.state, ParentOrderState::Filled);
    assert!((status.shortfall_bps.unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(engine.positions(Some("SIM")).positions[0].quantity, -300);
}

#[test]
fn cancel_stops_further_children() {
    let mut engine = engine(5, 1_000);
    let id = engine.submit(twap(Side::Buy, 1_000, 1, 11, 10), 0).unwrap();
    assert_eq!(total_filled(&run_seconds(&mut engine, 1, 1)), 100);

    engine.cancel(id).unwrap();
    assert!(run_seconds(&mut engine, 2, 12).is_empty());
    let status = engine.order_status(id).unwrap();
    assert_eq!(status.state, ParentOrderState::Cancelled);
    assert_eq!(status.state_reason.as_deref(), Some("cancelled by client"));
    assert_eq!(status.filled_quantity, 100);

    assert_eq!(
        engine.cancel(id),
        Err(CancelError::NotWorking {
            id,
            state: ParentOrderState::Cancelled
        })
    );
    assert_eq!(engine.cancel(999), Err(CancelError::Unknown(999)));
}

#[test]
fn invalid_orders_are_rejected_without_an_id() {
    let mut engine = engine(5, 1_000);
    let now = 5 * SECOND;

    let mut unknown = twap(Side::Buy, 10, 6, 7, 1);
    unknown.symbol = "NOPE".to_string();
    assert_eq!(
        engine.submit(unknown, now),
        Err(RejectReason::UnknownSymbol("NOPE".to_string()))
    );
    assert_eq!(
        engine.submit(twap(Side::Buy, 0, 6, 7, 1), now),
        Err(RejectReason::ZeroQuantity)
    );
    let mut negative_limit = twap(Side::Buy, 10, 6, 7, 1);
    negative_limit.limit_price = Some(Price::new(-1));
    assert_eq!(
        engine.submit(negative_limit, now),
        Err(RejectReason::NonPositiveLimitPrice)
    );
    assert_eq!(
        engine.submit(twap(Side::Buy, 10, 1, 5, 1), now),
        Err(RejectReason::WindowEnded)
    );
    assert!(matches!(
        engine.submit(twap(Side::Buy, 10, 6, 7, 0), now),
        Err(RejectReason::Schedule(_))
    ));
    let empty_profile = AlgorithmSpec::Vwap {
        volume_profile: vec![0.0, 0.0],
    };
    assert!(matches!(
        engine.submit(order(Side::Buy, 10, 6, 7, empty_profile), now),
        Err(RejectReason::Schedule(_))
    ));
    assert!(engine.order_status(1).is_none());
}

#[test]
fn risk_limits_are_checked_before_acceptance() {
    let limits = RiskLimits {
        max_order_qty: 1_000,
        max_position_qty: 1_000,
        price_collar_bps: 50,
        ..generous_limits()
    };
    let mut engine = engine_with(limits, 5, 1_000);

    assert!(matches!(
        engine.submit(twap(Side::Buy, 1_001, 1, 11, 10), 0),
        Err(RejectReason::Risk(RiskViolation::OrderQuantity { .. }))
    ));

    let mut outside_collar = twap(Side::Buy, 10, 1, 11, 10);
    outside_collar.limit_price = Some(Price::new(MID + MID / 100)); // 100 bps away
    assert!(matches!(
        engine.submit(outside_collar, 0),
        Err(RejectReason::Risk(RiskViolation::PriceCollar { .. }))
    ));

    // Working orders count toward the position limit before they fill.
    engine.submit(twap(Side::Buy, 600, 1, 11, 10), 0).unwrap();
    assert!(matches!(
        engine.submit(twap(Side::Buy, 600, 1, 11, 10), 0),
        Err(RejectReason::Risk(RiskViolation::PositionLimit { .. }))
    ));
    assert!(engine.submit(twap(Side::Sell, 600, 1, 11, 10), 0).is_ok());
}

#[test]
fn gross_notional_limit_counts_every_working_order() {
    let limits = RiskLimits {
        max_gross_notional: 1_000 * i128::from(MID),
        ..generous_limits()
    };
    let mut engine = engine_with(limits, 5, 1_000);
    engine.submit(twap(Side::Buy, 600, 1, 11, 10), 0).unwrap();
    assert!(matches!(
        engine.submit(twap(Side::Sell, 600, 1, 11, 10), 0),
        Err(RejectReason::Risk(RiskViolation::GrossNotional { .. }))
    ));
    assert!(engine.submit(twap(Side::Sell, 400, 1, 11, 10), 0).is_ok());
}

#[test]
fn kill_switch_halts_trading_and_cancels_working_orders() {
    // Buying 100 one level above the mid marks at -100 * STEP, past a 50_000 loss limit.
    let limits = RiskLimits {
        max_loss: 50_000,
        ..generous_limits()
    };
    let mut engine = engine_with(limits, 5, 1_000);
    let buy = engine.submit(twap(Side::Buy, 1_000, 1, 11, 10), 0).unwrap();
    let sell = engine.submit(twap(Side::Sell, 500, 5, 10, 5), 0).unwrap();

    assert_eq!(total_filled(&run_seconds(&mut engine, 1, 1)), 100);

    let reason = engine
        .halt_reason()
        .expect("the kill switch tripped")
        .to_string();
    for id in [buy, sell] {
        let status = engine.order_status(id).unwrap();
        assert_eq!(status.state, ParentOrderState::Cancelled);
        assert!(status.state_reason.unwrap().starts_with("kill switch"));
    }
    assert!(run_seconds(&mut engine, 2, 12).is_empty());
    assert_eq!(
        engine.submit(twap(Side::Sell, 100, 13, 14, 1), 12 * SECOND),
        Err(RejectReason::Halted(reason.clone()))
    );
    assert_eq!(engine.positions(None).halt_reason, Some(reason));
}

#[test]
fn a_round_trip_realizes_the_exact_spread_cost() {
    let mut engine = engine(5, 1_000);
    engine.submit(twap(Side::Buy, 100, 1, 2, 1), 0).unwrap();
    engine.submit(twap(Side::Sell, 100, 2, 3, 1), 0).unwrap();
    run_seconds(&mut engine, 1, 3);

    let position = &engine.positions(Some("SIM")).positions[0];
    assert_eq!(position.quantity, 0);
    // Bought at MID + STEP, sold at MID - STEP.
    assert_eq!(position.realized_pnl, -200 * i128::from(STEP));
    assert_eq!(position.unrealized_pnl, Some(0));
    assert_eq!(position.average_price_ticks, None);
}

#[test]
fn identical_inputs_produce_identical_results() {
    let scenario = || {
        let mut engine = engine(2, 70);
        engine.submit(twap(Side::Buy, 500, 1, 6, 5), 0).unwrap();
        let profile = AlgorithmSpec::Vwap {
            volume_profile: vec![1.0, 3.0, 2.0],
        };
        engine
            .submit(order(Side::Sell, 300, 2, 5, profile), 0)
            .unwrap();
        let fills = run_seconds(&mut engine, 0, 7);
        (fills, engine.positions(None))
    };
    assert_eq!(scenario(), scenario());
}

#[test]
fn implementation_shortfall_front_loads_relative_to_twap() {
    let mut engine = engine(20, 100_000);
    let twap_id = engine
        .submit(twap(Side::Buy, 12_000, 1, 13, 12), 0)
        .unwrap();
    let shortfall = AlgorithmSpec::ImplementationShortfall {
        num_slices: 12,
        risk_aversion: 1.0,
        volatility: 0.02,
        temporary_impact: 0.5,
        permanent_impact: 0.0,
    };
    let is_id = engine
        .submit(order(Side::Buy, 12_000, 1, 13, shortfall), 0)
        .unwrap();

    run_seconds(&mut engine, 1, 1);
    let first_child = |id| engine.order_status(id).unwrap().children[0].quantity;
    assert_eq!(first_child(twap_id), 1_000);
    assert!(first_child(is_id) > 1_000);
}

#[test]
fn vwap_children_follow_the_volume_profile() {
    let mut engine = engine(5, 10_000);
    let profile = AlgorithmSpec::Vwap {
        volume_profile: vec![3.0, 1.0],
    };
    let id = engine
        .submit(order(Side::Buy, 1_000, 1, 3, profile), 0)
        .unwrap();

    run_seconds(&mut engine, 1, 3);
    let status = engine.order_status(id).unwrap();
    let children: Vec<(u64, u64)> = status
        .children
        .iter()
        .map(|child| (child.sent_at_ns, child.quantity))
        .collect();
    assert_eq!(children, vec![(SECOND, 750), (2 * SECOND, 250)]);
    assert_eq!(status.state, ParentOrderState::Filled);
}

#[test]
fn slices_already_due_at_submission_are_sent_together() {
    let mut engine = engine(5, 10_000);
    let id = engine
        .submit(twap(Side::Buy, 1_000, 0, 10, 10), 5 * SECOND)
        .unwrap();

    run_seconds(&mut engine, 5, 5);
    let status = engine.order_status(id).unwrap();
    assert_eq!(status.children.len(), 1);
    assert_eq!(
        status.children[0].quantity, 600,
        "slices at 0 s through 5 s"
    );
    assert_eq!(status.pending_slices, 4);
}

#[test]
fn only_the_most_recent_finished_orders_are_retained() {
    let mut engine = EngineCore::new(config(generous_limits(), 5, 1_000, 2)).unwrap();
    let ids: Vec<u64> = (0..3)
        .map(|k| {
            engine
                .submit(twap(Side::Buy, 10, 1 + k, 2 + k, 1), 0)
                .unwrap()
        })
        .collect();

    run_seconds(&mut engine, 1, 5);
    assert!(engine.order_status(ids[0]).is_none());
    assert!(engine.order_status(ids[1]).is_some());
    assert!(engine.order_status(ids[2]).is_some());
}

#[test]
fn the_kill_switch_marks_losses_against_a_full_book() {
    // One level of 60 per side. The first child takes all 60 asks at MID + STEP.
    // The loss check used to run before quotes were refreshed, found no ask, so
    // no mid, and counted no unrealized loss; here the position reached 180
    // before any check saw the loss.
    let limits = RiskLimits {
        max_loss: 50_000,
        ..generous_limits()
    };
    let mut engine = engine_with(limits, 1, 60);
    let id = engine.submit(twap(Side::Buy, 300, 1, 4, 3), 0).unwrap();

    assert_eq!(total_filled(&run_seconds(&mut engine, 1, 1)), 60);

    // 60 bought at MID + STEP and marked at MID is -60 * STEP = -60_000.
    assert!(
        engine.halt_reason().is_some(),
        "the loss is past the limit after the first tick"
    );
    assert_eq!(
        engine.order_status(id).unwrap().state,
        ParentOrderState::Cancelled
    );
}

#[test]
fn queries_and_submissions_between_ticks_see_a_full_book() {
    let mut engine = engine(1, 60);
    engine.submit(twap(Side::Buy, 100, 1, 2, 1), 0).unwrap();
    run_seconds(&mut engine, 1, 1); // takes every ask

    let position = &engine.positions(Some("SIM")).positions[0];
    assert_eq!(position.mark_price, Some(Price::new(MID)));
    assert_eq!(position.unrealized_pnl, Some(-60 * i128::from(STEP)));
    // This used to be rejected with "no two-sided market", and gross exposure
    // left the drained symbol out entirely.
    assert!(engine.submit(twap(Side::Buy, 10, 2, 3, 1), SECOND).is_ok());
}

#[test]
fn slices_no_tick_reached_go_out_when_the_window_ends() {
    // Ticks only at 0 s, 5 s and 10 s. Slices 6 to 9 fall after the last tick
    // inside the window, and used to be dropped with the order expiring 40 short.
    let mut engine = engine(5, 10_000);
    let id = engine.submit(twap(Side::Buy, 100, 0, 10, 10), 0).unwrap();
    for second in [0, 5, 10] {
        engine.tick(second * SECOND);
    }

    let status = engine.order_status(id).unwrap();
    let children: Vec<(u64, u64)> = status
        .children
        .iter()
        .map(|child| (child.sent_at_ns, child.quantity))
        .collect();
    assert_eq!(children, vec![(0, 10), (5 * SECOND, 50), (10 * SECOND, 40)]);
    assert_eq!(status.state, ParentOrderState::Filled);
    assert_eq!(status.pending_slices, 0);
}

#[test]
fn a_child_that_fills_nothing_leaves_no_position() {
    let mut engine = engine(5, 1_000);
    let mut passive = twap(Side::Buy, 10, 1, 2, 1);
    passive.limit_price = Some(Price::new(MID)); // below the best ask
    engine.submit(passive, 0).unwrap();

    run_seconds(&mut engine, 1, 2);
    assert!(engine.positions(None).positions.is_empty());
}
