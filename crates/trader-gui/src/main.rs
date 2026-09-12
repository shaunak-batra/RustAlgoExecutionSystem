//! Desktop client for the execution engine.
//!
//! Every value on screen comes from the engine over gRPC: parent order state,
//! child orders, fills, positions, and whether risk has halted trading. This
//! process holds no market data source of its own and simulates nothing, so
//! anything displayed can be traced back to an engine reply.
//!
//! POV is deliberately absent. The order API offers TWAP, VWAP and
//! implementation shortfall; [`algo_core::pov`] exists and is tested, but the
//! simulated venue publishes no per-order volume feed for a participation rate
//! to track, so POV is reachable from the library and the Python bindings
//! rather than from here.

use api::proto::execution_service_client::ExecutionServiceClient;
use api::proto::submit_parent_order_request::Algorithm;
use api::proto::{
    CancelParentOrderRequest, ImplementationShortfallParams, OrderStatusRequest, ParentOrderState,
    PositionsRequest, StreamFillsRequest, SubmitParentOrderRequest, TwapParams, VwapParams,
};
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use orderbook::{Price, Side};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

/// Symbols in the shipped `config/default.toml`, offered as quick fills. The API
/// has no "list symbols" call, and `GetPositions` only reports symbols that have
/// traded, so these are a convenience rather than a source of truth.
const DEFAULT_SYMBOLS: [&str; 3] = ["BTC-USD", "ETH-USD", "SIM-EQ"];

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0)
}

// ============================================================================
// Conversions and form parsing. These are pure so they can be tested.
// ============================================================================

/// Converts a price in integer ticks to currency units.
fn ticks_to_price(ticks: i64) -> f64 {
    ticks as f64 / Price::TICK_SCALE
}

/// Converts an amount in ticks x units (how the engine reports money) to
/// currency units.
fn money_to_currency(ticks_times_units: i64) -> f64 {
    ticks_times_units as f64 / Price::TICK_SCALE
}

fn parse_quantity(text: &str) -> Result<u64, String> {
    match text.trim().parse::<u64>() {
        Ok(0) => Err("quantity must be greater than zero".to_string()),
        Ok(quantity) => Ok(quantity),
        Err(_) => Err(format!("quantity {text:?} is not a whole number")),
    }
}

fn parse_slices(text: &str) -> Result<u32, String> {
    match text.trim().parse::<u32>() {
        Ok(0) => Err("number of slices must be greater than zero".to_string()),
        Ok(slices) => Ok(slices),
        Err(_) => Err(format!("number of slices {text:?} is not a whole number")),
    }
}

/// Parses the limit price field. Empty, or a value of zero, means no limit: the
/// engine then sends market child orders.
fn parse_limit_price(text: &str) -> Result<Option<i64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let price: f64 = text
        .parse()
        .map_err(|_| format!("limit price {text:?} is not a number"))?;
    if price == 0.0 {
        return Ok(None);
    }
    if price < 0.0 {
        return Err("limit price must be positive, or zero for market".to_string());
    }
    Price::try_from_f64(price)
        .map(|price| Some(price.ticks()))
        .map_err(|error| error.to_string())
}

fn parse_duration_secs(text: &str) -> Result<u64, String> {
    let seconds: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("duration {text:?} is not a number"))?;
    if !(seconds.is_finite() && seconds > 0.0) {
        return Err("duration must be a positive number of seconds".to_string());
    }
    let nanos = seconds * 1e9;
    if nanos > u64::MAX as f64 {
        return Err("duration is too long".to_string());
    }
    Ok(nanos as u64)
}

/// Parses a comma or whitespace separated volume profile: one non-negative
/// weight per slice, at least one of them positive.
fn parse_volume_profile(text: &str) -> Result<Vec<f64>, String> {
    let mut weights = Vec::new();
    for field in text.split([',', ' ', '\t', '\n']) {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let weight: f64 = field
            .parse()
            .map_err(|_| format!("weight {field:?} is not a number"))?;
        if !weight.is_finite() || weight < 0.0 {
            return Err(format!("weight {field} must be finite and non-negative"));
        }
        weights.push(weight);
    }
    if weights.is_empty() {
        return Err("the volume profile needs one weight per slice".to_string());
    }
    if !weights.iter().any(|&w| w > 0.0) {
        return Err("the volume profile has no volume in it".to_string());
    }
    Ok(weights)
}

fn parse_positive(name: &str, text: &str) -> Result<f64, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{name} {text:?} is not a number"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{name} must be finite and positive"));
    }
    Ok(value)
}

fn parse_non_negative(name: &str, text: &str) -> Result<f64, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{name} {text:?} is not a number"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{name} must be finite and non-negative"));
    }
    Ok(value)
}

fn state_label(state: i32) -> &'static str {
    match ParentOrderState::try_from(state) {
        Ok(ParentOrderState::Working) => "WORKING",
        Ok(ParentOrderState::Filled) => "FILLED",
        Ok(ParentOrderState::Cancelled) => "CANCELLED",
        Ok(ParentOrderState::Expired) => "EXPIRED",
        _ => "UNKNOWN",
    }
}

fn side_label(side: i32) -> &'static str {
    match api::proto::Side::try_from(side) {
        Ok(api::proto::Side::Buy) => "BUY",
        Ok(api::proto::Side::Sell) => "SELL",
        _ => "?",
    }
}

/// Implementation shortfall against the arrival mid, in basis points. Positive
/// means worse than arrival, for either side.
fn format_shortfall(bps: Option<f64>) -> String {
    match bps {
        Some(bps) => format!("{bps:+.2} bps"),
        None => "-".to_string(),
    }
}

fn format_clock(timestamp_ns: u64) -> String {
    let seconds = timestamp_ns / 1_000_000_000 % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlgoChoice {
    Twap,
    Vwap,
    ImplementationShortfall,
}

impl AlgoChoice {
    fn label(&self) -> &'static str {
        match self {
            AlgoChoice::Twap => "TWAP",
            AlgoChoice::Vwap => "VWAP",
            AlgoChoice::ImplementationShortfall => "Implementation shortfall",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            AlgoChoice::Twap => {
                "Equal quantities per time slice, to within one unit. Slice k is \
                 released at start + k x duration / slices."
            }
            AlgoChoice::Vwap => {
                "Quantities in proportion to the volume profile below: one weight \
                 per slice, any scale. A slice with zero weight gets no order."
            }
            AlgoChoice::ImplementationShortfall => {
                "Almgren-Chriss optimal trajectory: minimises expected impact \
                 cost plus risk_aversion x variance. Zero risk aversion is TWAP."
            }
        }
    }
}

/// The order entry form, as typed.
struct OrderForm {
    symbol: String,
    side: Side,
    quantity: String,
    limit_price: String,
    duration_secs: String,
    algo: AlgoChoice,
    num_slices: String,
    volume_profile: String,
    risk_aversion: String,
    volatility: String,
    temporary_impact: String,
    permanent_impact: String,
}

impl Default for OrderForm {
    fn default() -> Self {
        Self {
            symbol: DEFAULT_SYMBOLS[0].to_string(),
            side: Side::Buy,
            quantity: "100".to_string(),
            limit_price: String::new(),
            duration_secs: "10".to_string(),
            algo: AlgoChoice::Twap,
            num_slices: "10".to_string(),
            volume_profile: "1, 2, 3, 2, 1".to_string(),
            risk_aversion: "0.001".to_string(),
            volatility: "0.02".to_string(),
            temporary_impact: "0.5".to_string(),
            permanent_impact: "0.000001".to_string(),
        }
    }
}

impl OrderForm {
    /// Builds the request, or returns the first problem with the form. The
    /// engine validates again and its rejection is shown verbatim; this only
    /// catches what can be checked without a round trip.
    fn to_request(&self, start_ns: u64) -> Result<SubmitParentOrderRequest, String> {
        if self.symbol.trim().is_empty() {
            return Err("symbol must not be empty".to_string());
        }
        let quantity = parse_quantity(&self.quantity)?;
        let limit_price_ticks = parse_limit_price(&self.limit_price)?;
        let duration_ns = parse_duration_secs(&self.duration_secs)?;

        let algorithm = match self.algo {
            AlgoChoice::Twap => Algorithm::Twap(TwapParams {
                num_slices: parse_slices(&self.num_slices)?,
            }),
            AlgoChoice::Vwap => Algorithm::Vwap(VwapParams {
                volume_profile: parse_volume_profile(&self.volume_profile)?,
            }),
            AlgoChoice::ImplementationShortfall => {
                Algorithm::ImplementationShortfall(ImplementationShortfallParams {
                    num_slices: parse_slices(&self.num_slices)?,
                    risk_aversion: parse_non_negative("risk aversion", &self.risk_aversion)?,
                    volatility: parse_non_negative("volatility", &self.volatility)?,
                    temporary_impact: parse_positive("temporary impact", &self.temporary_impact)?,
                    permanent_impact: parse_non_negative(
                        "permanent impact",
                        &self.permanent_impact,
                    )?,
                })
            }
        };

        Ok(SubmitParentOrderRequest {
            symbol: self.symbol.trim().to_string(),
            side: match self.side {
                Side::Buy => api::proto::Side::Buy as i32,
                Side::Sell => api::proto::Side::Sell as i32,
            },
            quantity,
            limit_price_ticks,
            start_time_ns: start_ns,
            end_time_ns: start_ns + duration_ns,
            algorithm: Some(algorithm),
        })
    }
}

// ============================================================================
// Engine state mirrored in the UI
// ============================================================================

/// A child order the engine reports under a parent.
#[derive(Clone, Debug)]
struct ChildRow {
    child_order_id: u64,
    sent_at_ns: u64,
    quantity: u64,
    filled_quantity: u64,
}

/// A parent order as last reported by `GetOrderStatus`.
#[derive(Clone, Debug)]
struct OrderRow {
    parent_order_id: u64,
    symbol: String,
    side: String,
    algorithm: String,
    state: String,
    state_reason: String,
    quantity: u64,
    filled_quantity: u64,
    limit_price: Option<f64>,
    arrival_mid: f64,
    average_fill_price: Option<f64>,
    shortfall_bps: Option<f64>,
    pending_slices: u32,
    children: Vec<ChildRow>,
}

impl OrderRow {
    fn is_terminal(&self) -> bool {
        self.state != "WORKING"
    }
}

#[derive(Clone, Debug)]
struct PositionRow {
    symbol: String,
    quantity: i64,
    average_price: Option<f64>,
    realized_pnl: f64,
    unrealized_pnl: Option<f64>,
    mark_price: Option<f64>,
}

#[derive(Clone, Debug)]
struct FillRow {
    parent_order_id: u64,
    child_order_id: u64,
    symbol: String,
    side: String,
    price: f64,
    quantity: u64,
    timestamp_ns: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Clone, Debug)]
struct LogEntry {
    at: String,
    message: String,
}

#[derive(Clone)]
struct AppState {
    status: Arc<Mutex<String>>,
    connection: Arc<Mutex<ConnectionStatus>>,
    orders: Arc<Mutex<Vec<OrderRow>>>,
    positions: Arc<Mutex<Vec<PositionRow>>>,
    /// `Some(reason)` when the engine has halted trading on its loss limit.
    halt_reason: Arc<Mutex<Option<String>>>,
    fills: Arc<Mutex<VecDeque<FillRow>>>,
    logs: Arc<Mutex<VecDeque<LogEntry>>>,
    /// Every parent order id the engine has accepted this session. The status
    /// poll works from this list, so an id has to land here as soon as it exists.
    tracked_orders: Arc<Mutex<Vec<u64>>>,
    client: Arc<Mutex<Option<ExecutionServiceClient<Channel>>>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl AppState {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            status: Arc::new(Mutex::new("Not connected".to_string())),
            connection: Arc::new(Mutex::new(ConnectionStatus::Disconnected)),
            orders: Arc::new(Mutex::new(Vec::new())),
            positions: Arc::new(Mutex::new(Vec::new())),
            halt_reason: Arc::new(Mutex::new(None)),
            fills: Arc::new(Mutex::new(VecDeque::new())),
            logs: Arc::new(Mutex::new(VecDeque::new())),
            tracked_orders: Arc::new(Mutex::new(Vec::new())),
            client: Arc::new(Mutex::new(None)),
            runtime: Arc::new(runtime),
        }
    }

    fn log(&self, message: impl Into<String>) {
        let message = message.into();
        let entry = LogEntry {
            at: format_clock(now_ns()),
            message: message.clone(),
        };
        let mut logs = self.logs.lock().unwrap();
        logs.push_back(entry);
        while logs.len() > 500 {
            logs.pop_front();
        }
        eprintln!("[gui] {message}");
    }

    fn set_status(&self, message: impl Into<String>) {
        *self.status.lock().unwrap() = message.into();
    }

    fn connected_client(&self) -> Option<ExecutionServiceClient<Channel>> {
        self.client.lock().unwrap().clone()
    }

    /// Records an accepted parent order id so the status poll picks it up.
    fn track_order(&self, parent_order_id: u64) {
        let mut tracked = self.tracked_orders.lock().unwrap();
        if !tracked.contains(&parent_order_id) {
            tracked.push(parent_order_id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Orders,
    Positions,
    Fills,
    Connection,
}

impl Page {
    fn name(&self) -> &'static str {
        match self {
            Page::Orders => "Orders",
            Page::Positions => "Positions",
            Page::Fills => "Fills",
            Page::Connection => "Connection",
        }
    }
}

struct TraderApp {
    page: Page,
    server_address: String,
    form: OrderForm,
    last_error: Option<String>,
    selected_order: Option<u64>,
    state: AppState,
    last_poll_secs: f64,
}

impl Default for TraderApp {
    fn default() -> Self {
        Self {
            page: Page::Orders,
            // Matches [api].listen_addr in config/default.toml.
            server_address: "127.0.0.1:50051".to_string(),
            form: OrderForm::default(),
            last_error: None,
            selected_order: None,
            state: AppState::new(),
            last_poll_secs: 0.0,
        }
    }
}

impl eframe::App for TraderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint steadily so streamed fills appear without user input.
        ctx.request_repaint_after(Duration::from_millis(250));

        let now_secs = now_ns() as f64 / 1e9;
        if now_secs - self.last_poll_secs > 0.5
            && *self.state.connection.lock().unwrap() == ConnectionStatus::Connected
        {
            self.last_poll_secs = now_secs;
            self.refresh_positions();
            self.refresh_working_orders();
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Execution engine client");
                ui.separator();
                for page in [Page::Orders, Page::Positions, Page::Fills, Page::Connection] {
                    if ui
                        .selectable_label(self.page == page, page.name())
                        .clicked()
                    {
                        self.page = page;
                    }
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let connection = self.state.connection.lock().unwrap().clone();
                let (colour, text) = match &connection {
                    ConnectionStatus::Connected => (egui::Color32::GREEN, "connected".to_string()),
                    ConnectionStatus::Connecting => {
                        (egui::Color32::YELLOW, "connecting".to_string())
                    }
                    ConnectionStatus::Disconnected => {
                        (egui::Color32::GRAY, "not connected".to_string())
                    }
                    ConnectionStatus::Error(error) => (egui::Color32::RED, error.clone()),
                };
                ui.colored_label(colour, format!("● {text}"));
                ui.separator();
                ui.label(self.state.status.lock().unwrap().clone());

                if let Some(reason) = self.state.halt_reason.lock().unwrap().clone() {
                    ui.separator();
                    ui.colored_label(egui::Color32::RED, format!("TRADING HALTED: {reason}"));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Orders => self.render_orders(ui),
            Page::Positions => self.render_positions(ui),
            Page::Fills => self.render_fills(ui),
            Page::Connection => self.render_connection(ui),
        });
    }
}

impl TraderApp {
    // ------------------------------------------------------------------ pages

    fn render_orders(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.group(|ui| {
                ui.label(egui::RichText::new("New parent order").strong());
                egui::Grid::new("order_form")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Symbol");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.form.symbol);
                            for symbol in DEFAULT_SYMBOLS {
                                if ui.small_button(symbol).clicked() {
                                    self.form.symbol = symbol.to_string();
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Side");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.form.side, Side::Buy, "buy");
                            ui.radio_value(&mut self.form.side, Side::Sell, "sell");
                        });
                        ui.end_row();

                        ui.label("Quantity");
                        ui.text_edit_singleline(&mut self.form.quantity);
                        ui.end_row();

                        ui.label("Limit price");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.form.limit_price);
                            ui.label(
                                egui::RichText::new("empty or 0 sends market child orders")
                                    .small()
                                    .italics(),
                            );
                        });
                        ui.end_row();

                        ui.label("Duration (s)");
                        ui.text_edit_singleline(&mut self.form.duration_secs);
                        ui.end_row();

                        ui.label("Algorithm");
                        ui.horizontal(|ui| {
                            for algo in [
                                AlgoChoice::Twap,
                                AlgoChoice::Vwap,
                                AlgoChoice::ImplementationShortfall,
                            ] {
                                ui.radio_value(&mut self.form.algo, algo, algo.label());
                            }
                        });
                        ui.end_row();
                    });

                ui.label(
                    egui::RichText::new(self.form.algo.description())
                        .italics()
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(4.0);

                egui::Grid::new("algo_params")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| match self.form.algo {
                        AlgoChoice::Twap => {
                            ui.label("Slices");
                            ui.text_edit_singleline(&mut self.form.num_slices);
                            ui.end_row();
                        }
                        AlgoChoice::Vwap => {
                            ui.label("Volume profile");
                            ui.text_edit_singleline(&mut self.form.volume_profile);
                            ui.end_row();
                        }
                        AlgoChoice::ImplementationShortfall => {
                            ui.label("Slices");
                            ui.text_edit_singleline(&mut self.form.num_slices);
                            ui.end_row();
                            ui.label("Risk aversion (lambda)");
                            ui.text_edit_singleline(&mut self.form.risk_aversion);
                            ui.end_row();
                            ui.label("Volatility (sigma)");
                            ui.text_edit_singleline(&mut self.form.volatility);
                            ui.end_row();
                            ui.label("Temporary impact (eta)");
                            ui.text_edit_singleline(&mut self.form.temporary_impact);
                            ui.end_row();
                            ui.label("Permanent impact (gamma)");
                            ui.text_edit_singleline(&mut self.form.permanent_impact);
                            ui.end_row();
                        }
                    });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let connected =
                        *self.state.connection.lock().unwrap() == ConnectionStatus::Connected;
                    if ui
                        .add_enabled(connected, egui::Button::new("Submit"))
                        .clicked()
                    {
                        self.submit_order();
                    }
                    if !connected {
                        ui.label(
                            egui::RichText::new("connect to the engine first")
                                .small()
                                .italics(),
                        );
                    }
                });

                if let Some(error) = &self.last_error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            });

            ui.add_space(10.0);
            ui.label(egui::RichText::new("Parent orders").strong());

            let orders = self.state.orders.lock().unwrap().clone();
            if orders.is_empty() {
                ui.label("No orders submitted yet.");
                return;
            }

            egui::Grid::new("orders")
                .striped(true)
                .num_columns(9)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    for header in [
                        "id",
                        "symbol",
                        "side",
                        "algorithm",
                        "filled",
                        "avg fill",
                        "arrival mid",
                        "shortfall",
                        "state",
                    ] {
                        ui.strong(header);
                    }
                    ui.end_row();

                    for order in &orders {
                        if ui
                            .selectable_label(
                                self.selected_order == Some(order.parent_order_id),
                                format!("#{}", order.parent_order_id),
                            )
                            .clicked()
                        {
                            self.selected_order = Some(order.parent_order_id);
                        }
                        ui.label(&order.symbol);
                        ui.label(&order.side);
                        ui.label(&order.algorithm);
                        ui.label(format!("{}/{}", order.filled_quantity, order.quantity));
                        match order.average_fill_price {
                            Some(price) => ui.label(format!("{price:.5}")),
                            None => ui.label("-"),
                        };
                        ui.label(format!("{:.5}", order.arrival_mid));
                        ui.label(format_shortfall(order.shortfall_bps));

                        let colour = match order.state.as_str() {
                            "FILLED" => egui::Color32::GREEN,
                            "WORKING" => egui::Color32::LIGHT_BLUE,
                            "CANCELLED" | "EXPIRED" => egui::Color32::YELLOW,
                            _ => egui::Color32::GRAY,
                        };
                        ui.colored_label(colour, &order.state);
                        ui.end_row();
                    }
                });

            let selected = self
                .selected_order
                .and_then(|id| orders.iter().find(|order| order.parent_order_id == id));
            if let Some(order) = selected {
                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Order #{}", order.parent_order_id)).strong(),
                    );
                    if !order.state_reason.is_empty() {
                        ui.label(format!("reason: {}", order.state_reason));
                    }
                    ui.label(match order.limit_price {
                        Some(price) => format!("limit {price:.5}"),
                        None => "market child orders".to_string(),
                    });
                    ui.label(format!("slices still to release: {}", order.pending_slices));

                    if !order.is_terminal() && ui.button("Cancel remaining quantity").clicked() {
                        self.cancel_order(order.parent_order_id);
                    }

                    ui.add_space(6.0);
                    ui.label("Child orders");
                    egui::Grid::new("children")
                        .striped(true)
                        .num_columns(4)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            for header in ["child", "sent", "quantity", "filled"] {
                                ui.strong(header);
                            }
                            ui.end_row();
                            for child in &order.children {
                                ui.label(format!("#{}", child.child_order_id));
                                ui.label(format_clock(child.sent_at_ns));
                                ui.label(child.quantity.to_string());
                                ui.label(child.filled_quantity.to_string());
                                ui.end_row();
                            }
                        });
                });
            }
        });
    }

    fn render_positions(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Positions").strong());
        ui.label(
            egui::RichText::new(
                "Realised and unrealised amounts are exact integers in the engine \
                 (price ticks x units) and are shown here in currency units.",
            )
            .small()
            .italics(),
        );
        ui.add_space(6.0);

        let positions = self.state.positions.lock().unwrap().clone();
        if positions.is_empty() {
            ui.label("No positions. They appear once an order fills.");
            return;
        }

        egui::Grid::new("positions")
            .striped(true)
            .num_columns(6)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for header in [
                    "symbol",
                    "quantity",
                    "avg price",
                    "mark",
                    "realised",
                    "unrealised",
                ] {
                    ui.strong(header);
                }
                ui.end_row();

                for position in &positions {
                    ui.label(&position.symbol);
                    ui.label(position.quantity.to_string());
                    match position.average_price {
                        Some(price) => ui.label(format!("{price:.5}")),
                        None => ui.label("-"),
                    };
                    match position.mark_price {
                        Some(price) => ui.label(format!("{price:.5}")),
                        None => ui.label("-"),
                    };
                    let realised_colour = if position.realized_pnl >= 0.0 {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::RED
                    };
                    ui.colored_label(realised_colour, format!("{:.5}", position.realized_pnl));
                    match position.unrealized_pnl {
                        Some(pnl) => {
                            let colour = if pnl >= 0.0 {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(colour, format!("{pnl:.5}"))
                        }
                        None => ui.label("-"),
                    };
                    ui.end_row();
                }
            });
    }

    fn render_fills(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Fills").strong());
        ui.label(
            egui::RichText::new(
                "Streamed from the engine. If the stream falls behind it ends with \
                 DATA_LOSS and says so, rather than dropping fills silently.",
            )
            .small()
            .italics(),
        );
        ui.add_space(6.0);

        let fills: Vec<FillRow> = self.state.fills.lock().unwrap().iter().cloned().collect();
        if fills.is_empty() {
            ui.label("No fills yet.");
            return;
        }

        let points: PlotPoints = fills
            .iter()
            .enumerate()
            .map(|(index, fill)| [index as f64, fill.price])
            .collect();
        Plot::new("fill_prices")
            .height(160.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |plot_ui| {
                plot_ui.line(Line::new(points).name("fill price"));
            });

        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("fills")
                .striped(true)
                .num_columns(6)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for header in ["time", "parent", "child", "symbol", "side", "price x qty"] {
                        ui.strong(header);
                    }
                    ui.end_row();
                    for fill in fills.iter().rev() {
                        ui.label(format_clock(fill.timestamp_ns));
                        ui.label(format!("#{}", fill.parent_order_id));
                        ui.label(format!("#{}", fill.child_order_id));
                        ui.label(&fill.symbol);
                        ui.label(&fill.side);
                        ui.label(format!("{:.5} x {}", fill.price, fill.quantity));
                        ui.end_row();
                    }
                });
        });
    }

    fn render_connection(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Engine connection").strong());
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.text_edit_singleline(&mut self.server_address);
        });
        ui.horizontal(|ui| {
            if ui.button("Connect").clicked() {
                self.connect();
            }
            if ui.button("Disconnect").clicked() {
                *self.state.client.lock().unwrap() = None;
                *self.state.connection.lock().unwrap() = ConnectionStatus::Disconnected;
                self.state.set_status("Disconnected");
            }
        });

        ui.add_space(10.0);
        ui.label(egui::RichText::new("Log").strong());
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in self.state.logs.lock().unwrap().iter() {
                    ui.label(format!("{}  {}", entry.at, entry.message));
                }
            });
    }

    // --------------------------------------------------------------- requests

    fn connect(&mut self) {
        let address = self.server_address.clone();
        let state = self.state.clone();
        *state.connection.lock().unwrap() = ConnectionStatus::Connecting;
        state.set_status(format!("Connecting to {address}"));

        let runtime = state.runtime.clone();
        thread::spawn(move || {
            let endpoint = format!("http://{address}");
            match runtime.block_on(ExecutionServiceClient::connect(endpoint)) {
                Ok(client) => {
                    *state.client.lock().unwrap() = Some(client);
                    *state.connection.lock().unwrap() = ConnectionStatus::Connected;
                    state.set_status(format!("Connected to {address}"));
                    state.log(format!("connected to {address}"));
                    stream_fills(state.clone());
                }
                Err(error) => {
                    *state.connection.lock().unwrap() = ConnectionStatus::Error(format!("{error}"));
                    state.set_status("Connection failed");
                    state.log(format!("connection to {address} failed: {error}"));
                }
            }
        });
    }

    fn submit_order(&mut self) {
        let request = match self.form.to_request(now_ns()) {
            Ok(request) => request,
            Err(error) => {
                self.last_error = Some(error);
                return;
            }
        };
        self.last_error = None;

        let Some(mut client) = self.state.connected_client() else {
            self.last_error = Some("not connected to the engine".to_string());
            return;
        };

        let state = self.state.clone();
        let runtime = state.runtime.clone();
        let summary = format!(
            "{} {} {}",
            side_label(request.side),
            request.quantity,
            request.symbol
        );
        thread::spawn(move || {
            match runtime.block_on(client.submit_parent_order(request)) {
                Ok(response) => {
                    let response = response.into_inner();
                    if response.accepted {
                        let id = response.parent_order_id;
                        state.log(format!("order #{id} accepted: {summary}"));
                        state.set_status(format!("Order #{id} working"));
                        // Track the id first. The status poll works from this
                        // list, so an order that is accepted but never tracked
                        // would never be polled and would stay invisible.
                        state.track_order(id);
                        // Then read it back, so the table shows engine state now
                        // rather than after the next poll.
                        match runtime.block_on(client.get_order_status(OrderStatusRequest {
                            parent_order_id: id,
                        })) {
                            Ok(status) => update_order(&state, status.into_inner()),
                            Err(status) => state.log(format!(
                                "order #{id} accepted, but reading its status failed: {}",
                                status.message()
                            )),
                        }
                    } else {
                        // A business rejection, not a transport failure: the
                        // engine replies with the reason.
                        state.log(format!("order rejected: {}", response.reject_reason));
                        state.set_status(format!("Rejected: {}", response.reject_reason));
                    }
                }
                Err(status) => {
                    state.log(format!(
                        "submit failed: {} ({})",
                        status.message(),
                        status.code()
                    ));
                    state.set_status(format!("Submit failed: {}", status.message()));
                }
            }
        });
    }

    fn cancel_order(&mut self, parent_order_id: u64) {
        let Some(mut client) = self.state.connected_client() else {
            return;
        };
        let state = self.state.clone();
        let runtime = state.runtime.clone();
        thread::spawn(move || {
            let request = CancelParentOrderRequest { parent_order_id };
            match runtime.block_on(client.cancel_parent_order(request)) {
                Ok(response) => {
                    let response = response.into_inner();
                    if response.cancelled {
                        state.log(format!("order #{parent_order_id} cancelled"));
                    } else {
                        state.log(format!(
                            "order #{parent_order_id} not cancelled: {}",
                            response.reason
                        ));
                    }
                }
                Err(status) => state.log(format!("cancel failed: {}", status.message())),
            }
        });
    }

    fn refresh_positions(&self) {
        let Some(mut client) = self.state.connected_client() else {
            return;
        };
        let state = self.state.clone();
        let runtime = state.runtime.clone();
        thread::spawn(move || {
            let request = PositionsRequest {
                symbol: String::new(),
            };
            match runtime.block_on(client.get_positions(request)) {
                Ok(response) => {
                    let response = response.into_inner();
                    let rows = response
                        .positions
                        .into_iter()
                        .map(|position| PositionRow {
                            symbol: position.symbol,
                            quantity: position.quantity,
                            average_price: position
                                .average_price_ticks
                                .map(|ticks| ticks / Price::TICK_SCALE),
                            realized_pnl: money_to_currency(position.realized_pnl),
                            unrealized_pnl: position.unrealized_pnl.map(money_to_currency),
                            mark_price: position.mark_price_ticks.map(ticks_to_price),
                        })
                        .collect();
                    *state.positions.lock().unwrap() = rows;
                    *state.halt_reason.lock().unwrap() = response
                        .trading_halted
                        .then_some(response.halt_reason)
                        .filter(|reason| !reason.is_empty());
                }
                Err(status) => state.log(format!("positions query failed: {}", status.message())),
            }
        });
    }

    /// Re-reads every tracked order that is not finished, including one the UI
    /// holds no status for at all.
    fn refresh_working_orders(&self) {
        let Some(mut client) = self.state.connected_client() else {
            return;
        };
        let ids = {
            // tracked_orders before orders, which is the only order these two are
            // ever taken in.
            let tracked = self.state.tracked_orders.lock().unwrap();
            let orders = self.state.orders.lock().unwrap();
            ids_to_poll(&tracked, &orders)
        };
        if ids.is_empty() {
            return;
        }

        let state = self.state.clone();
        let runtime = state.runtime.clone();
        thread::spawn(move || {
            for parent_order_id in ids {
                let request = OrderStatusRequest { parent_order_id };
                match runtime.block_on(client.get_order_status(request)) {
                    Ok(response) => update_order(&state, response.into_inner()),
                    Err(status) => state.log(format!(
                        "status of #{parent_order_id} failed: {}",
                        status.message()
                    )),
                }
            }
        });
    }
}

/// Which tracked orders still need a status read: those with no row yet, and
/// those whose last known state was not terminal. An accepted order the UI has
/// never seen has to be polled, or it would never appear at all.
fn ids_to_poll(tracked: &[u64], orders: &[OrderRow]) -> Vec<u64> {
    tracked
        .iter()
        .copied()
        .filter(
            |id| match orders.iter().find(|order| order.parent_order_id == *id) {
                Some(order) => !order.is_terminal(),
                // Accepted, but no status reply has landed yet.
                None => true,
            },
        )
        .collect()
}

/// Stores a status reply, replacing any earlier copy of the same order.
fn update_order(state: &AppState, status: api::proto::OrderStatusResponse) {
    let row = OrderRow {
        parent_order_id: status.parent_order_id,
        symbol: status.symbol,
        side: side_label(status.side).to_string(),
        algorithm: status.algorithm,
        state: state_label(status.state).to_string(),
        state_reason: status.state_reason,
        quantity: status.quantity,
        filled_quantity: status.filled_quantity,
        limit_price: status.limit_price_ticks.map(ticks_to_price),
        arrival_mid: ticks_to_price(status.arrival_mid_ticks),
        average_fill_price: status
            .average_fill_price_ticks
            .map(|ticks| ticks / Price::TICK_SCALE),
        shortfall_bps: status.shortfall_bps,
        pending_slices: status.pending_slices,
        children: status
            .children
            .into_iter()
            .map(|child| ChildRow {
                child_order_id: child.child_order_id,
                sent_at_ns: child.sent_at_ns,
                quantity: child.quantity,
                filled_quantity: child.filled_quantity,
            })
            .collect(),
    };

    let mut orders = state.orders.lock().unwrap();
    match orders
        .iter_mut()
        .find(|existing| existing.parent_order_id == row.parent_order_id)
    {
        Some(existing) => *existing = row,
        None => orders.insert(0, row),
    }
}

/// Subscribes to the fill stream for as long as the connection lasts.
fn stream_fills(state: AppState) {
    let Some(mut client) = state.connected_client() else {
        return;
    };
    let runtime = state.runtime.clone();
    thread::spawn(move || {
        let stream = runtime.block_on(client.stream_fills(StreamFillsRequest {}));
        let mut stream = match stream {
            Ok(response) => response.into_inner(),
            Err(status) => {
                state.log(format!("fill stream refused: {}", status.message()));
                return;
            }
        };
        state.log("subscribed to the fill stream");

        loop {
            match runtime.block_on(stream.message()) {
                Ok(Some(fill)) => {
                    let row = FillRow {
                        parent_order_id: fill.parent_order_id,
                        child_order_id: fill.child_order_id,
                        symbol: fill.symbol,
                        side: side_label(fill.side).to_string(),
                        price: ticks_to_price(fill.price_ticks),
                        quantity: fill.quantity,
                        timestamp_ns: fill.timestamp_ns,
                    };
                    let mut fills = state.fills.lock().unwrap();
                    fills.push_back(row);
                    while fills.len() > 500 {
                        fills.pop_front();
                    }
                }
                // The engine shut the stream down cleanly.
                Ok(None) => {
                    state.log("fill stream ended");
                    return;
                }
                Err(status) => {
                    // DATA_LOSS means this subscriber fell behind; the engine
                    // says to reconcile with GetOrderStatus, which the poll does.
                    state.log(format!(
                        "fill stream ended: {} ({})",
                        status.message(),
                        status.code()
                    ));
                    return;
                }
            }
        }
    });
}

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Execution engine client",
        options,
        Box::new(|_cc| Ok(Box::<TraderApp>::default())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_must_be_positive_whole_numbers() {
        assert_eq!(parse_quantity(" 1000 "), Ok(1_000));
        assert!(parse_quantity("0").is_err());
        assert!(parse_quantity("-5").is_err());
        assert!(parse_quantity("10.5").is_err());
        assert!(parse_quantity("").is_err());
    }

    #[test]
    fn an_empty_or_zero_limit_price_means_market() {
        assert_eq!(parse_limit_price(""), Ok(None));
        assert_eq!(parse_limit_price("   "), Ok(None));
        assert_eq!(parse_limit_price("0"), Ok(None));
        assert_eq!(parse_limit_price("0.0"), Ok(None));
    }

    #[test]
    fn limit_prices_convert_to_ticks_not_whole_units() {
        // The earlier client sent 50000 here, understating the price 100_000x.
        assert_eq!(parse_limit_price("50000"), Ok(Some(5_000_000_000)));
        assert_eq!(parse_limit_price("0.29"), Ok(Some(29_000)));
        assert!(parse_limit_price("-1").is_err());
        assert!(parse_limit_price("abc").is_err());
        assert!(parse_limit_price("1e300").is_err());
    }

    #[test]
    fn durations_must_be_positive() {
        assert_eq!(parse_duration_secs("10"), Ok(10_000_000_000));
        assert_eq!(parse_duration_secs("0.5"), Ok(500_000_000));
        assert!(parse_duration_secs("0").is_err());
        assert!(parse_duration_secs("-1").is_err());
        assert!(parse_duration_secs("nan").is_err());
    }

    #[test]
    fn volume_profiles_accept_commas_or_spaces() {
        assert_eq!(parse_volume_profile("1,2,3"), Ok(vec![1.0, 2.0, 3.0]));
        assert_eq!(parse_volume_profile(" 1  2 "), Ok(vec![1.0, 2.0]));
        assert_eq!(parse_volume_profile("1, 0, 2"), Ok(vec![1.0, 0.0, 2.0]));
        assert!(parse_volume_profile("").is_err());
        assert!(parse_volume_profile("0, 0").is_err(), "no volume at all");
        assert!(parse_volume_profile("1, -1").is_err());
        assert!(parse_volume_profile("1, x").is_err());
    }

    fn form() -> OrderForm {
        OrderForm::default()
    }

    #[test]
    fn twap_request_carries_the_slice_count() {
        let request = form().to_request(1_000).unwrap();
        assert_eq!(request.symbol, "BTC-USD");
        assert_eq!(request.side, api::proto::Side::Buy as i32);
        assert_eq!(request.quantity, 100);
        assert_eq!(request.limit_price_ticks, None);
        assert_eq!(request.start_time_ns, 1_000);
        assert_eq!(request.end_time_ns, 1_000 + 10_000_000_000);
        assert_eq!(
            request.algorithm,
            Some(Algorithm::Twap(TwapParams { num_slices: 10 }))
        );
    }

    #[test]
    fn vwap_request_carries_the_profile() {
        let mut form = form();
        form.algo = AlgoChoice::Vwap;
        form.volume_profile = "3, 1".to_string();
        let request = form.to_request(0).unwrap();
        assert_eq!(
            request.algorithm,
            Some(Algorithm::Vwap(VwapParams {
                volume_profile: vec![3.0, 1.0]
            }))
        );
    }

    #[test]
    fn shortfall_request_carries_every_parameter() {
        let mut form = form();
        form.algo = AlgoChoice::ImplementationShortfall;
        form.side = Side::Sell;
        form.num_slices = "12".to_string();
        let request = form.to_request(0).unwrap();
        assert_eq!(request.side, api::proto::Side::Sell as i32);
        assert_eq!(
            request.algorithm,
            Some(Algorithm::ImplementationShortfall(
                ImplementationShortfallParams {
                    num_slices: 12,
                    risk_aversion: 0.001,
                    volatility: 0.02,
                    temporary_impact: 0.5,
                    permanent_impact: 0.000001,
                }
            ))
        );
    }

    #[test]
    fn risk_neutral_shortfall_is_allowed_but_a_zero_eta_is_not() {
        let mut form = form();
        form.algo = AlgoChoice::ImplementationShortfall;
        form.risk_aversion = "0".to_string();
        assert!(form.to_request(0).is_ok(), "lambda = 0 is TWAP");

        form.temporary_impact = "0".to_string();
        assert!(form.to_request(0).is_err(), "eta must be positive");
    }

    #[test]
    fn a_bad_field_is_reported_rather_than_sent() {
        let mut form = form();
        form.quantity = "lots".to_string();
        let error = form.to_request(0).unwrap_err();
        assert!(error.contains("quantity"), "{error}");

        let mut form = self::form();
        form.symbol = "  ".to_string();
        assert!(form.to_request(0).unwrap_err().contains("symbol"));
    }

    #[test]
    fn money_and_prices_are_scaled_by_the_tick_size() {
        assert_eq!(ticks_to_price(5_000_000_000), 50_000.0);
        assert_eq!(money_to_currency(-250_000), -2.5);
        assert_eq!(ticks_to_price(0), 0.0);
    }

    #[test]
    fn state_and_side_labels_cover_every_variant() {
        assert_eq!(state_label(ParentOrderState::Working as i32), "WORKING");
        assert_eq!(state_label(ParentOrderState::Filled as i32), "FILLED");
        assert_eq!(state_label(ParentOrderState::Cancelled as i32), "CANCELLED");
        assert_eq!(state_label(ParentOrderState::Expired as i32), "EXPIRED");
        assert_eq!(state_label(99), "UNKNOWN");

        assert_eq!(side_label(api::proto::Side::Buy as i32), "BUY");
        assert_eq!(side_label(api::proto::Side::Sell as i32), "SELL");
        assert_eq!(side_label(0), "?");
    }

    #[test]
    fn shortfall_shows_its_sign_and_absence() {
        assert_eq!(format_shortfall(Some(12.5)), "+12.50 bps");
        assert_eq!(format_shortfall(Some(-3.0)), "-3.00 bps");
        assert_eq!(format_shortfall(None), "-");
    }

    #[test]
    fn clock_formatting_wraps_at_a_day() {
        assert_eq!(format_clock(0), "00:00:00");
        assert_eq!(format_clock(3_661 * 1_000_000_000), "01:01:01");
        assert_eq!(format_clock(86_400 * 1_000_000_000), "00:00:00");
    }

    fn row(parent_order_id: u64, order_state: &str) -> OrderRow {
        OrderRow {
            parent_order_id,
            symbol: "BTC-USD".to_string(),
            side: "BUY".to_string(),
            algorithm: "TWAP".to_string(),
            state: order_state.to_string(),
            state_reason: String::new(),
            quantity: 100,
            filled_quantity: 0,
            limit_price: None,
            arrival_mid: 0.0,
            average_fill_price: None,
            shortfall_bps: None,
            pending_slices: 0,
            children: Vec::new(),
        }
    }

    #[test]
    fn an_accepted_order_is_polled_before_any_status_has_arrived() {
        // The bug this pins: submitting recorded nothing, and the poll took its
        // ids from the orders table, so an accepted order was never polled and
        // never showed up in the UI at all.
        assert_eq!(ids_to_poll(&[7], &[]), vec![7]);
    }

    #[test]
    fn polling_stops_once_an_order_is_finished() {
        assert_eq!(ids_to_poll(&[7], &[row(7, "WORKING")]), vec![7]);
        for finished in ["FILLED", "CANCELLED", "EXPIRED"] {
            assert!(
                ids_to_poll(&[7], &[row(7, finished)]).is_empty(),
                "{finished} should not be polled again"
            );
        }
    }

    #[test]
    fn polling_covers_every_unfinished_tracked_order_once() {
        let orders = [row(1, "FILLED"), row(2, "WORKING")];
        assert_eq!(ids_to_poll(&[1, 2, 3], &orders), vec![2, 3]);
    }

    #[test]
    fn tracking_the_same_order_twice_keeps_one_entry() {
        let state = AppState::new();
        state.track_order(5);
        state.track_order(5);
        state.track_order(6);
        assert_eq!(*state.tracked_orders.lock().unwrap(), vec![5, 6]);
    }

    #[test]
    fn a_status_reply_replaces_the_earlier_copy_of_that_order() {
        let state = AppState::new();
        let reply = |filled, order_state: ParentOrderState| api::proto::OrderStatusResponse {
            parent_order_id: 7,
            symbol: "BTC-USD".to_string(),
            side: api::proto::Side::Buy as i32,
            algorithm: "TWAP".to_string(),
            state: order_state as i32,
            state_reason: String::new(),
            quantity: 100,
            filled_quantity: filled,
            limit_price_ticks: None,
            start_time_ns: 0,
            end_time_ns: 1,
            arrival_mid_ticks: 5_000_000_000,
            average_fill_price_ticks: Some(5_000_000_000.0),
            shortfall_bps: Some(1.5),
            pending_slices: 3,
            children: Vec::new(),
        };

        update_order(&state, reply(40, ParentOrderState::Working));
        update_order(&state, reply(100, ParentOrderState::Filled));

        let orders = state.orders.lock().unwrap();
        assert_eq!(orders.len(), 1, "the same order must not be listed twice");
        assert_eq!(orders[0].filled_quantity, 100);
        assert_eq!(orders[0].state, "FILLED");
        assert!(orders[0].is_terminal());
        assert_eq!(orders[0].average_fill_price, Some(50_000.0));
    }
}
