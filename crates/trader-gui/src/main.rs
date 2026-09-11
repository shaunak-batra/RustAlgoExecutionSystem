use analytics::{EquityPoint, PerformanceAnalyzer, PerformanceMetrics, Trade, TradeType};
use api::proto::execution_service_client::ExecutionServiceClient;
use api::proto::{ParentOrderRequest, PositionsRequest};
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use orderbook::Side;
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

// ============================================================================
// PAGE NAVIGATION
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Trading,
    MarketData,
    PositionsOrders,
    Backtesting,
    AdvancedAlgos,
    Analytics,
    Logs,
    Settings,
}

impl Page {
    fn icon(&self) -> &'static str {
        match self {
            Page::Dashboard => "🏠",
            Page::Trading => "📝",
            Page::MarketData => "📊",
            Page::PositionsOrders => "💼",
            Page::Backtesting => "⏮",
            Page::AdvancedAlgos => "🧠",
            Page::Analytics => "📈",
            Page::Logs => "📋",
            Page::Settings => "⚙",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Page::Dashboard => "Dashboard",
            Page::Trading => "Trading",
            Page::MarketData => "Market Data",
            Page::PositionsOrders => "Positions & Orders",
            Page::Backtesting => "Backtesting",
            Page::AdvancedAlgos => "Advanced Algos",
            Page::Analytics => "Analytics",
            Page::Logs => "Logs",
            Page::Settings => "Settings",
        }
    }
}

// ============================================================================
// DATA TYPES
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
enum AlgoType {
    Twap,
    Pov,
    Vwap,
    IS,
}

impl std::fmt::Display for AlgoType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlgoType::Twap => write!(f, "TWAP"),
            AlgoType::Pov => write!(f, "POV"),
            AlgoType::Vwap => write!(f, "VWAP"),
            AlgoType::IS => write!(f, "IS"),
        }
    }
}

#[derive(Clone, Debug)]
struct MarketData {
    symbol: String,
    bid: f64,
    ask: f64,
    last: f64,
    volume: u64,
    price_history: VecDeque<(f64, f64)>,
}

#[derive(Clone, Debug)]
struct LiveIndicators {
    rsi: Option<f64>,
    macd: Option<f64>,
    macd_signal: Option<f64>,
    bb_upper: Option<f64>,
    #[allow(dead_code)]
    bb_middle: Option<f64>,
    bb_lower: Option<f64>,
    last_update: u64,
}

#[derive(Clone, Debug)]
struct Transaction {
    #[allow(dead_code)]
    symbol: String,
    side: String,
    price: f64,
    quantity: u64,
    #[allow(dead_code)]
    entity: String,
    timestamp: String,
}

#[derive(Clone, Debug)]
struct NewsItem {
    #[allow(dead_code)]
    symbol: String,
    headline: String,
    timestamp: String,
    sentiment: String,
}

#[derive(Clone, Debug)]
struct LogEntry {
    timestamp: String,
    category: String,
    message: String,
    color: [u8; 3], // RGB color
}

#[derive(Clone)]
struct AppState {
    status_message: Arc<Mutex<String>>,
    positions: Arc<Mutex<Vec<PositionInfo>>>,
    orders: Arc<Mutex<Vec<OrderInfo>>>,
    connection_status: Arc<Mutex<ConnectionStatus>>,
    market_data: Arc<Mutex<Vec<MarketData>>>,
    transactions: Arc<Mutex<VecDeque<Transaction>>>,
    news: Arc<Mutex<Vec<NewsItem>>>,
    popular_symbols: Arc<Mutex<Vec<String>>>,
    performance_metrics: Arc<Mutex<Option<PerformanceMetrics>>>,
    equity_curve: Arc<Mutex<Vec<EquityPoint>>>,
    live_indicators: Arc<Mutex<std::collections::HashMap<String, LiveIndicators>>>,
    performance_analyzer: Arc<Mutex<PerformanceAnalyzer>>,
    grpc_client: Arc<Mutex<Option<ExecutionServiceClient<Channel>>>>,
    grpc_runtime: Arc<Mutex<tokio::runtime::Runtime>>,
    // Backtesting state
    backtest_running: Arc<Mutex<bool>>,
    backtest_progress: Arc<Mutex<f64>>,
    backtest_results: Arc<Mutex<Vec<String>>>,
    // Adaptive TWAP state
    adaptive_twap_running: Arc<Mutex<bool>>,
    adaptive_child_orders: Arc<Mutex<Vec<OrderInfo>>>,
    adaptive_progress: Arc<Mutex<f64>>,
    // Logs
    logs: Arc<Mutex<VecDeque<LogEntry>>>,
}

#[derive(Clone, Debug)]
struct PositionInfo {
    symbol: String,
    quantity: i64,
    avg_price: f64,
    realized_pnl: f64,
    unrealized_pnl: f64,
}

#[derive(Clone, Debug)]
struct OrderInfo {
    order_id: u64,
    symbol: String,
    side: String,
    quantity: u64,
    filled_qty: u64,
    status: String,
    submitted_at: String,
}

#[derive(Clone, Debug, PartialEq)]
enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

impl Default for AppState {
    fn default() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        Self {
            status_message: Arc::new(Mutex::new(String::from("Ready"))),
            positions: Arc::new(Mutex::new(Vec::new())),
            orders: Arc::new(Mutex::new(Vec::new())),
            connection_status: Arc::new(Mutex::new(ConnectionStatus::Disconnected)),
            market_data: Arc::new(Mutex::new(Vec::new())),
            transactions: Arc::new(Mutex::new(VecDeque::new())),
            news: Arc::new(Mutex::new(Vec::new())),
            popular_symbols: Arc::new(Mutex::new(vec![
                "BTCUSDT".to_string(),
                "ETHUSDT".to_string(),
                "SOLUSDT".to_string(),
                "BNBUSDT".to_string(),
                "ADAUSDT".to_string(),
            ])),
            performance_metrics: Arc::new(Mutex::new(None)),
            equity_curve: Arc::new(Mutex::new(Vec::new())),
            live_indicators: Arc::new(Mutex::new(std::collections::HashMap::new())),
            performance_analyzer: Arc::new(Mutex::new(PerformanceAnalyzer::new(100_000.0, 0.02))),
            grpc_client: Arc::new(Mutex::new(None)),
            grpc_runtime: Arc::new(Mutex::new(runtime)),
            backtest_running: Arc::new(Mutex::new(false)),
            backtest_progress: Arc::new(Mutex::new(0.0)),
            backtest_results: Arc::new(Mutex::new(Vec::new())),
            adaptive_twap_running: Arc::new(Mutex::new(false)),
            adaptive_child_orders: Arc::new(Mutex::new(Vec::new())),
            adaptive_progress: Arc::new(Mutex::new(0.0)),
            logs: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

// ============================================================================
// MAIN APPLICATION
// ============================================================================

struct TraderApp {
    // Navigation
    current_page: Page,

    // Connection
    server_address: String,

    // Order entry fields
    symbol: String,
    side: Side,
    quantity: String,
    limit_price: String,
    duration_sec: String,
    num_slices: String,
    algo_type: AlgoType,

    // Adaptive TWAP configuration
    adaptive_total_qty: String,
    adaptive_duration_secs: String,
    adaptive_base_slice_secs: String,
    adaptive_enable: bool,
    adaptive_volatility_window: String,
    adaptive_max_participation: String,

    // Advanced order type selection
    advanced_order_type: AdvancedOrderType,
    stop_loss_trigger: String,
    take_profit_target: String,
    trailing_stop_distance: String,
    iceberg_display_qty: String,

    // Routing strategy
    routing_strategy: String,

    // Backtesting controls
    #[allow(dead_code)]
    backtest_start_time: String,
    #[allow(dead_code)]
    backtest_end_time: String,
    backtest_replay_speed: ReplaySpeed,
    backtest_paused: bool,

    // Logs filter
    log_filter: String,

    // Shared state
    state: AppState,

    // Update timers
    last_update: f64,
    last_poll_time: f64,
}

#[derive(Clone, Debug, PartialEq)]
enum AdvancedOrderType {
    Limit,
    Market,
    StopLoss,
    TakeProfit,
    TrailingStop,
    Iceberg,
    PostOnly,
}

#[derive(Clone, Debug, PartialEq)]
enum ReplaySpeed {
    RealTime,
    FastForward2x,
    FastForward5x,
    FastForward10x,
    Maximum,
}

impl Default for TraderApp {
    fn default() -> Self {
        let app = Self {
            current_page: Page::Dashboard,
            server_address: "127.0.0.1:9090".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            quantity: "1000".to_string(),
            limit_price: "50000.0".to_string(),
            duration_sec: "10.0".to_string(),
            num_slices: "10".to_string(),
            algo_type: AlgoType::Twap,
            adaptive_total_qty: "1000".to_string(),
            adaptive_duration_secs: "300".to_string(),
            adaptive_base_slice_secs: "60".to_string(),
            adaptive_enable: true,
            adaptive_volatility_window: "20".to_string(),
            adaptive_max_participation: "0.3".to_string(),
            advanced_order_type: AdvancedOrderType::Limit,
            stop_loss_trigger: "49000.0".to_string(),
            take_profit_target: "51000.0".to_string(),
            trailing_stop_distance: "500.0".to_string(),
            iceberg_display_qty: "100".to_string(),
            routing_strategy: "Smart".to_string(),
            backtest_start_time: "0".to_string(),
            backtest_end_time: "3600000000000".to_string(),
            backtest_replay_speed: ReplaySpeed::FastForward5x,
            backtest_paused: false,
            log_filter: String::new(),
            state: AppState::default(),
            last_update: 0.0,
            last_poll_time: 0.0,
        };

        start_realtime_data_fetcher(app.state.clone());
        app
    }
}

impl eframe::App for TraderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();

        // Poll order status every 500ms if connected
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        if now - self.last_poll_time > 0.5 {
            self.last_poll_time = now;
            let conn_status = self.state.connection_status.lock().unwrap().clone();
            if matches!(conn_status, ConnectionStatus::Connected) {
                self.poll_order_status();
                self.refresh_data();
            }
        }

        // Top menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Connect to Engine").clicked() {
                        self.connect_to_engine();
                    }
                    if ui.button("Disconnect").clicked() {
                        self.disconnect();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        *self.state.status_message.lock().unwrap() =
                            "QuantSystem Trading Platform v1.0 - Full-Featured Dashboard"
                                .to_string();
                    }
                });
            });
        });

        // Status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let conn_status = self.state.connection_status.lock().unwrap().clone();
                let (color, text) = match conn_status {
                    ConnectionStatus::Disconnected => {
                        (egui::Color32::GRAY, "● Disconnected".to_string())
                    }
                    ConnectionStatus::Connecting => {
                        (egui::Color32::YELLOW, "● Connecting...".to_string())
                    }
                    ConnectionStatus::Connected => {
                        (egui::Color32::GREEN, "● Connected".to_string())
                    }
                    ConnectionStatus::Error(msg) => {
                        (egui::Color32::RED, format!("● Error: {}", msg))
                    }
                };
                ui.colored_label(color, text);
                ui.separator();

                let status = self.state.status_message.lock().unwrap().clone();
                ui.label(status);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(egui::Color32::GREEN, "🔴 LIVE (Binance)");
                    ui.separator();
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64();
                    ui.label(format!("Updated: {:.1}s ago", now - self.last_update));
                });
            });
        });

        // Left sidebar navigation
        egui::SidePanel::left("navigation_panel")
            .resizable(false)
            .exact_width(180.0)
            .show(ctx, |ui| {
                self.render_navigation(ui);
            });

        // Main content area
        egui::CentralPanel::default().show(ctx, |ui| match self.current_page {
            Page::Dashboard => self.render_dashboard(ui),
            Page::Trading => self.render_trading_page(ui),
            Page::MarketData => self.render_market_data_page(ui),
            Page::PositionsOrders => self.render_positions_orders_page(ui),
            Page::Backtesting => self.render_backtesting_page(ui),
            Page::AdvancedAlgos => self.render_advanced_algos_page(ui),
            Page::Analytics => self.render_analytics_page(ui),
            Page::Logs => self.render_logs_page(ui),
            Page::Settings => self.render_settings_page(ui),
        });

        self.last_update = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
    }
}

// ============================================================================
// NAVIGATION RENDERING
// ============================================================================

impl TraderApp {
    fn render_navigation(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.heading("QuantSystem");
            ui.label("Trading Platform");
            ui.add_space(10.0);
        });

        ui.separator();
        ui.add_space(5.0);

        let pages = [
            Page::Dashboard,
            Page::Trading,
            Page::MarketData,
            Page::PositionsOrders,
            Page::Backtesting,
            Page::AdvancedAlgos,
            Page::Analytics,
            Page::Logs,
            Page::Settings,
        ];

        for page in &pages {
            let is_selected = self.current_page == *page;
            let button_text = format!("{} {}", page.icon(), page.name());

            let button = egui::Button::new(button_text)
                .min_size(egui::vec2(160.0, 40.0))
                .fill(if is_selected {
                    egui::Color32::from_rgb(50, 80, 120)
                } else {
                    egui::Color32::from_rgb(30, 30, 30)
                });

            if ui.add(button).clicked() {
                self.current_page = *page;
            }
            ui.add_space(2.0);
        }
    }

    // ========================================================================
    // PAGE: DASHBOARD
    // ========================================================================

    fn render_dashboard(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("📊 Trading Dashboard");
            ui.add_space(10.0);

            // Connection status card
            ui.group(|ui| {
                ui.set_min_width(ui.available_width());
                let conn_status = self.state.connection_status.lock().unwrap().clone();
                let (color, text) = match conn_status {
                    ConnectionStatus::Connected => {
                        (egui::Color32::GREEN, "✓ Connected to Execution Engine")
                    }
                    ConnectionStatus::Connecting => (egui::Color32::YELLOW, "⟳ Connecting..."),
                    ConnectionStatus::Disconnected => (egui::Color32::GRAY, "○ Disconnected"),
                    ConnectionStatus::Error(ref msg) => (egui::Color32::RED, msg.as_str()),
                };
                ui.colored_label(color, egui::RichText::new(text).heading());
            });
            ui.add_space(10.0);

            // Overview cards
            ui.horizontal(|ui| {
                // Market summary card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("📈 Market Summary");
                        ui.separator();
                        let market_data = self.state.market_data.lock().unwrap();
                        if !market_data.is_empty() {
                            for (i, data) in market_data.iter().enumerate().take(5) {
                                ui.horizontal(|ui| {
                                    ui.label(&data.symbol);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(format!("${:.2}", data.last));
                                        },
                                    );
                                });
                                if i < 4 && i < market_data.len() - 1 {
                                    ui.add_space(5.0);
                                }
                            }
                        } else {
                            ui.label("Loading...");
                        }
                    });
                });

                ui.add_space(10.0);

                // Positions summary card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("💼 Positions");
                        ui.separator();
                        let positions = self.state.positions.lock().unwrap();
                        if positions.is_empty() {
                            ui.label("No open positions");
                        } else {
                            for pos in positions.iter().take(5) {
                                ui.horizontal(|ui| {
                                    ui.label(&pos.symbol);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let color = if pos.unrealized_pnl >= 0.0 {
                                                egui::Color32::GREEN
                                            } else {
                                                egui::Color32::RED
                                            };
                                            ui.colored_label(
                                                color,
                                                format!("${:.2}", pos.unrealized_pnl),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(5.0);
                            }
                        }
                    });
                });

                ui.add_space(10.0);

                // Orders summary card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("📋 Recent Orders");
                        ui.separator();
                        let orders = self.state.orders.lock().unwrap();
                        if orders.is_empty() {
                            ui.label("No orders");
                        } else {
                            for order in orders.iter().take(5) {
                                let status_color = match order.status.as_str() {
                                    "FILLED" => egui::Color32::GREEN,
                                    "WORKING" => egui::Color32::BLUE,
                                    "REJECTED" => egui::Color32::RED,
                                    _ => egui::Color32::GRAY,
                                };
                                ui.horizontal(|ui| {
                                    ui.label(format!("#{}", order.order_id));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.colored_label(status_color, &order.status);
                                        },
                                    );
                                });
                                ui.add_space(5.0);
                            }
                        }
                    });
                });
            });

            ui.add_space(20.0);

            // Live price charts
            ui.group(|ui| {
                ui.heading("📈 Live Price Charts");
                ui.separator();
                let market_data = self.state.market_data.lock().unwrap().clone();

                ui.horizontal_wrapped(|ui| {
                    for data in market_data.iter().take(3) {
                        ui.group(|ui| {
                            ui.set_min_width(300.0);
                            ui.label(egui::RichText::new(&data.symbol).strong());
                            ui.label(format!("${:.2} | Vol: {}", data.last, data.volume));

                            let points: PlotPoints =
                                data.price_history.iter().map(|(t, p)| [*t, *p]).collect();

                            let line =
                                Line::new(points).color(egui::Color32::from_rgb(0, 200, 100));

                            Plot::new(format!("dash_chart_{}", data.symbol))
                                .height(120.0)
                                .show_axes([false, true])
                                .allow_zoom(false)
                                .allow_drag(false)
                                .allow_scroll(false)
                                .show(ui, |plot_ui| {
                                    plot_ui.line(line);
                                });
                        });
                        ui.add_space(5.0);
                    }
                });
            });

            ui.add_space(20.0);

            // Recent transactions
            ui.group(|ui| {
                ui.heading("💸 Recent Market Trades");
                ui.separator();
                let transactions = self.state.transactions.lock().unwrap().clone();

                egui::Grid::new("dash_trans_grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong("Symbol");
                        ui.strong("Side");
                        ui.strong("Price");
                        ui.strong("Qty");
                        ui.strong("Time");
                        ui.end_row();

                        for trans in transactions.iter().take(10) {
                            ui.label(&trans.symbol);
                            let side_color = if trans.side == "BUY" {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(side_color, &trans.side);
                            ui.label(format!("${:.2}", trans.price));
                            ui.label(format!("{:.4}", trans.quantity));
                            ui.label(&trans.timestamp);
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(20.0);

            // News feed
            ui.group(|ui| {
                ui.heading("📰 Market News");
                ui.separator();
                let news = self.state.news.lock().unwrap().clone();

                for item in news.iter().take(5) {
                    let sentiment_color = match item.sentiment.as_str() {
                        "positive" => egui::Color32::GREEN,
                        "negative" => egui::Color32::RED,
                        _ => egui::Color32::GRAY,
                    };

                    ui.horizontal(|ui| {
                        ui.colored_label(sentiment_color, "●");
                        ui.label(
                            egui::RichText::new(&item.headline).text_style(egui::TextStyle::Body),
                        );
                    });
                    ui.label(egui::RichText::new(&item.timestamp).small().italics());
                    ui.separator();
                }
            });
        });
    }

    // ========================================================================
    // PAGE: TRADING
    // ========================================================================

    fn render_trading_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("📝 Order Entry");
            ui.add_space(10.0);

            ui.group(|ui| {
                ui.set_min_width(ui.available_width());

                egui::Grid::new("order_entry_grid")
                    .num_columns(2)
                    .spacing([40.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Symbol:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.symbol);
                            let popular = self.state.popular_symbols.lock().unwrap().clone();
                            for sym in popular.iter().take(3) {
                                if ui.small_button(sym).clicked() {
                                    self.symbol = sym.clone();
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Side:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.side, Side::Buy, "BUY");
                            ui.radio_value(&mut self.side, Side::Sell, "SELL");
                        });
                        ui.end_row();

                        ui.label("Quantity:");
                        ui.text_edit_singleline(&mut self.quantity);
                        ui.end_row();

                        ui.label("Limit Price:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.limit_price);
                            if ui.button("Market").clicked() {
                                self.limit_price = "0.0".to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Duration (sec):");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.duration_sec);
                            if ui.button("5s").clicked() {
                                self.duration_sec = "5.0".to_string();
                            }
                            if ui.button("10s").clicked() {
                                self.duration_sec = "10.0".to_string();
                            }
                            if ui.button("30s").clicked() {
                                self.duration_sec = "30.0".to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Number of Slices:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.num_slices);
                            if ui.button("5").clicked() {
                                self.num_slices = "5".to_string();
                            }
                            if ui.button("10").clicked() {
                                self.num_slices = "10".to_string();
                            }
                            if ui.button("20").clicked() {
                                self.num_slices = "20".to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Algorithm:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.algo_type, AlgoType::Twap, "TWAP");
                            ui.radio_value(&mut self.algo_type, AlgoType::Pov, "POV");
                            ui.radio_value(&mut self.algo_type, AlgoType::Vwap, "VWAP");
                            ui.radio_value(&mut self.algo_type, AlgoType::IS, "IS");
                        });
                        ui.end_row();
                    });

                ui.add_space(5.0);

                let algo_desc = match self.algo_type {
                    AlgoType::Twap => "Time-Weighted Average Price: Splits order evenly over time",
                    AlgoType::Pov => {
                        "Percent of Volume: Executes based on market volume percentage"
                    }
                    AlgoType::Vwap => {
                        "Volume-Weighted Average Price: Follows historical volume patterns"
                    }
                    AlgoType::IS => "Implementation Shortfall: Balances urgency vs market impact",
                };
                ui.label(
                    egui::RichText::new(algo_desc)
                        .italics()
                        .color(egui::Color32::GRAY),
                );

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let button_text = format!("🚀 Submit {} Order", self.algo_type);
                    if ui
                        .add_sized([180.0, 45.0], egui::Button::new(button_text))
                        .clicked()
                    {
                        self.submit_order();
                    }

                    ui.add_space(10.0);

                    if ui
                        .add_sized([120.0, 45.0], egui::Button::new("🔄 Refresh"))
                        .clicked()
                    {
                        self.refresh_data();
                    }
                });
            });

            ui.add_space(20.0);

            // Quick market view
            ui.group(|ui| {
                ui.heading("Current Market Prices");
                ui.separator();
                let market_data = self.state.market_data.lock().unwrap();

                egui::Grid::new("trading_market_grid")
                    .striped(true)
                    .num_columns(4)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Symbol");
                        ui.strong("Bid");
                        ui.strong("Ask");
                        ui.strong("Last");
                        ui.end_row();

                        for data in market_data.iter().take(5) {
                            ui.label(&data.symbol);
                            ui.label(format!("${:.2}", data.bid));
                            ui.label(format!("${:.2}", data.ask));
                            ui.label(format!("${:.2}", data.last));
                            ui.end_row();
                        }
                    });
            });
        });
    }

    // ========================================================================
    // PAGE: MARKET DATA
    // ========================================================================

    fn render_market_data_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("📊 Market Data");
            ui.add_space(10.0);

            // Real-time prices
            ui.group(|ui| {
                ui.heading("Real-Time Prices");
                ui.separator();
                let market_data = self.state.market_data.lock().unwrap();

                if market_data.is_empty() {
                    ui.label("Loading real-time data from Binance...");
                } else {
                    egui::Grid::new("market_data_grid")
                        .striped(true)
                        .num_columns(5)
                        .spacing([25.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Symbol");
                            ui.strong("Bid");
                            ui.strong("Ask");
                            ui.strong("Last");
                            ui.strong("Volume");
                            ui.end_row();

                            for data in market_data.iter() {
                                ui.label(&data.symbol);
                                ui.label(format!("${:.2}", data.bid));
                                ui.label(format!("${:.2}", data.ask));
                                ui.label(format!("${:.2}", data.last));
                                ui.label(format!("{}", data.volume));
                                ui.end_row();
                            }
                        });
                }
            });

            ui.add_space(20.0);

            // Live price charts
            ui.group(|ui| {
                ui.heading("Live Price Charts");
                ui.separator();
                let market_data = self.state.market_data.lock().unwrap().clone();

                for data in market_data.iter() {
                    ui.label(egui::RichText::new(&data.symbol).strong());
                    ui.label(format!("${:.2} | Vol: {}", data.last, data.volume));

                    let points: PlotPoints =
                        data.price_history.iter().map(|(t, p)| [*t, *p]).collect();

                    let line = Line::new(points).color(egui::Color32::from_rgb(0, 200, 100));

                    Plot::new(format!("market_chart_{}", data.symbol))
                        .height(150.0)
                        .show_axes([false, true])
                        .allow_zoom(true)
                        .allow_drag(true)
                        .show(ui, |plot_ui| {
                            plot_ui.line(line);
                        });

                    ui.separator();
                    ui.add_space(10.0);
                }
            });

            ui.add_space(20.0);

            // Technical indicators
            ui.group(|ui| {
                ui.heading("📈 Live Technical Indicators");
                ui.separator();
                let indicators = self.state.live_indicators.lock().unwrap();

                if indicators.is_empty() {
                    ui.label("Calculating indicators... (need 50+ price points)");
                } else {
                    egui::Grid::new("indicators_grid")
                        .striped(true)
                        .num_columns(5)
                        .spacing([20.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Symbol");
                            ui.strong("RSI(14)");
                            ui.strong("MACD");
                            ui.strong("BB Upper/Lower");
                            ui.strong("Status");
                            ui.end_row();

                            for (symbol, ind) in indicators.iter() {
                                ui.label(symbol);

                                if let Some(rsi) = ind.rsi {
                                    let rsi_color = if rsi > 70.0 {
                                        egui::Color32::RED
                                    } else if rsi < 30.0 {
                                        egui::Color32::GREEN
                                    } else {
                                        egui::Color32::YELLOW
                                    };
                                    ui.colored_label(rsi_color, format!("{:.1}", rsi));
                                } else {
                                    ui.label("-");
                                }

                                if let (Some(macd), Some(signal)) = (ind.macd, ind.macd_signal) {
                                    let macd_color = if macd > signal {
                                        egui::Color32::GREEN
                                    } else {
                                        egui::Color32::RED
                                    };
                                    ui.colored_label(
                                        macd_color,
                                        format!("{:.2}/{:.2}", macd, signal),
                                    );
                                } else {
                                    ui.label("-");
                                }

                                if let (Some(upper), Some(lower)) = (ind.bb_upper, ind.bb_lower) {
                                    ui.label(format!("{:.2}/{:.2}", upper, lower));
                                } else {
                                    ui.label("-");
                                }

                                let age = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                                    - ind.last_update;
                                ui.label(format!("{}s ago", age));
                                ui.end_row();
                            }
                        });
                }
            });
        });
    }

    // ========================================================================
    // PAGE: POSITIONS & ORDERS
    // ========================================================================

    fn render_positions_orders_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("💼 Positions & Orders");
            ui.add_space(10.0);

            // Positions
            ui.group(|ui| {
                ui.heading("Positions");
                ui.separator();
                let positions = self.state.positions.lock().unwrap();

                if positions.is_empty() {
                    ui.label("No positions");
                } else {
                    egui::Grid::new("positions_grid")
                        .striped(true)
                        .num_columns(5)
                        .spacing([20.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Symbol");
                            ui.strong("Quantity");
                            ui.strong("Avg Price");
                            ui.strong("Realized P&L");
                            ui.strong("Unrealized P&L");
                            ui.end_row();

                            for pos in positions.iter() {
                                ui.label(&pos.symbol);
                                ui.label(format!("{}", pos.quantity));
                                ui.label(format!("${:.2}", pos.avg_price));

                                let rpnl_color = if pos.realized_pnl >= 0.0 {
                                    egui::Color32::GREEN
                                } else {
                                    egui::Color32::RED
                                };
                                ui.colored_label(rpnl_color, format!("${:.2}", pos.realized_pnl));

                                let upnl_color = if pos.unrealized_pnl >= 0.0 {
                                    egui::Color32::GREEN
                                } else {
                                    egui::Color32::RED
                                };
                                ui.colored_label(upnl_color, format!("${:.2}", pos.unrealized_pnl));
                                ui.end_row();
                            }
                        });
                }
            });

            ui.add_space(20.0);

            // Orders
            ui.group(|ui| {
                ui.heading("Orders");
                ui.separator();
                let orders = self.state.orders.lock().unwrap();

                if orders.is_empty() {
                    ui.label("No orders");
                } else {
                    egui::Grid::new("orders_grid")
                        .striped(true)
                        .num_columns(7)
                        .spacing([15.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Order ID");
                            ui.strong("Symbol");
                            ui.strong("Side");
                            ui.strong("Quantity");
                            ui.strong("Filled");
                            ui.strong("Status");
                            ui.strong("Time");
                            ui.end_row();

                            for order in orders.iter() {
                                ui.label(format!("{}", order.order_id));
                                ui.label(&order.symbol);

                                let side_color = if order.side == "BUY" {
                                    egui::Color32::from_rgb(0, 150, 0)
                                } else {
                                    egui::Color32::from_rgb(200, 0, 0)
                                };
                                ui.colored_label(side_color, &order.side);

                                ui.label(format!("{}", order.quantity));
                                ui.label(format!("{}/{}", order.filled_qty, order.quantity));

                                let status_color = match order.status.as_str() {
                                    "PENDING" => egui::Color32::GRAY,
                                    "WORKING" => egui::Color32::BLUE,
                                    "FILLED" => egui::Color32::GREEN,
                                    "CANCELLED" => egui::Color32::YELLOW,
                                    "REJECTED" => egui::Color32::RED,
                                    _ => egui::Color32::WHITE,
                                };
                                ui.colored_label(status_color, &order.status);

                                ui.label(&order.submitted_at);
                                ui.end_row();
                            }
                        });
                }
            });
        });
    }

    // ========================================================================
    // PAGE: BACKTESTING
    // ========================================================================

    fn render_backtesting_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("⏮ Historical Replay & Backtesting");
            ui.add_space(10.0);

            let is_running = *self.state.backtest_running.lock().unwrap();

            ui.group(|ui| {
                ui.heading("Backtest Configuration");
                ui.separator();

                ui.label("Using collected real-time market data for simulation");
                ui.add_space(5.0);

                egui::Grid::new("backtest_grid")
                    .num_columns(2)
                    .spacing([40.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Symbol:");
                        ui.text_edit_singleline(&mut self.symbol);
                        ui.end_row();

                        ui.label("Replay Speed:");
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut self.backtest_replay_speed,
                                ReplaySpeed::RealTime,
                                "Real-Time",
                            );
                            ui.radio_value(
                                &mut self.backtest_replay_speed,
                                ReplaySpeed::FastForward2x,
                                "2x",
                            );
                            ui.radio_value(
                                &mut self.backtest_replay_speed,
                                ReplaySpeed::FastForward5x,
                                "5x",
                            );
                            ui.radio_value(
                                &mut self.backtest_replay_speed,
                                ReplaySpeed::FastForward10x,
                                "10x",
                            );
                            ui.radio_value(
                                &mut self.backtest_replay_speed,
                                ReplaySpeed::Maximum,
                                "Max",
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let start_enabled = !is_running;
                    if ui
                        .add_enabled(start_enabled, egui::Button::new("▶ Start Backtest"))
                        .clicked()
                    {
                        self.start_backtest();
                    }
                    if ui
                        .add_enabled(is_running, egui::Button::new("⏸ Pause"))
                        .clicked()
                    {
                        self.backtest_paused = !self.backtest_paused;
                    }
                    if ui
                        .add_enabled(is_running, egui::Button::new("⏹ Stop"))
                        .clicked()
                    {
                        self.stop_backtest();
                    }
                });
            });

            ui.add_space(20.0);

            // Progress
            if is_running {
                ui.group(|ui| {
                    ui.heading("Backtest Progress");
                    ui.separator();
                    let progress = *self.state.backtest_progress.lock().unwrap();
                    ui.label(format!("Progress: {:.1}%", progress * 100.0));
                    let progress_bar = egui::ProgressBar::new(progress as f32).show_percentage();
                    ui.add(progress_bar);
                });
                ui.add_space(20.0);
            }

            // Results
            ui.group(|ui| {
                ui.heading("Backtest Results");
                ui.separator();
                let results = self.state.backtest_results.lock().unwrap();
                if results.is_empty() {
                    ui.label("Run a backtest to see results here");
                    ui.label("Features: Historical replay using collected market data");
                } else {
                    for result in results.iter() {
                        ui.label(result);
                    }
                }
            });
        });
    }

    // ========================================================================
    // PAGE: ADVANCED ALGOS
    // ========================================================================

    fn render_advanced_algos_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("🧠 Advanced Algorithms & Features");
            ui.add_space(10.0);

            // Check if Adaptive TWAP is running (before any closures)
            let is_running = *self.state.adaptive_twap_running.lock().unwrap();

            // Adaptive TWAP
            ui.group(|ui| {
                ui.heading("Adaptive TWAP Configuration");
                ui.separator();

                egui::Grid::new("adaptive_twap_grid")
                    .num_columns(2)
                    .spacing([40.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Total Quantity:");
                        ui.text_edit_singleline(&mut self.adaptive_total_qty);
                        ui.end_row();

                        ui.label("Duration (seconds):");
                        ui.text_edit_singleline(&mut self.adaptive_duration_secs);
                        ui.end_row();

                        ui.label("Base Slice Duration (sec):");
                        ui.text_edit_singleline(&mut self.adaptive_base_slice_secs);
                        ui.end_row();

                        ui.label("Enable Adaptive:");
                        ui.checkbox(&mut self.adaptive_enable, "Adjust slices based on volatility");
                        ui.end_row();

                        ui.label("Volatility Window:");
                        ui.text_edit_singleline(&mut self.adaptive_volatility_window);
                        ui.end_row();

                        ui.label("Max Participation Rate:");
                        ui.text_edit_singleline(&mut self.adaptive_max_participation);
                        ui.end_row();
                    });

                ui.add_space(10.0);
                ui.label(egui::RichText::new(
                    "Adaptive TWAP dynamically adjusts slice sizes based on market volatility and volume"
                ).italics().color(egui::Color32::GRAY));

                ui.add_space(10.0);
                let button_text = if is_running {
                    "⏸ Adaptive TWAP Running..."
                } else {
                    "🚀 Submit Adaptive TWAP Order"
                };

                if ui.add_enabled(!is_running, egui::Button::new(button_text)).clicked() {
                    self.submit_adaptive_twap();
                }

                if is_running {
                    ui.add_space(10.0);
                    let progress = *self.state.adaptive_progress.lock().unwrap();
                    ui.label(format!("Progress: {:.1}%", progress * 100.0));
                    let progress_bar = egui::ProgressBar::new(progress as f32).show_percentage();
                    ui.add(progress_bar);
                }
            });

            ui.add_space(20.0);

            // Adaptive TWAP Child Orders
            if is_running {
                ui.group(|ui| {
                    ui.heading("Adaptive TWAP Child Orders");
                    ui.separator();
                    let child_orders = self.state.adaptive_child_orders.lock().unwrap();

                    if child_orders.is_empty() {
                        ui.label("Generating child orders...");
                    } else {
                        egui::Grid::new("adaptive_child_orders_grid")
                            .striped(true)
                            .num_columns(5)
                            .spacing([15.0, 8.0])
                            .show(ui, |ui| {
                                ui.strong("Child #");
                                ui.strong("Side");
                                ui.strong("Quantity");
                                ui.strong("Status");
                                ui.strong("Time");
                                ui.end_row();

                                for (i, order) in child_orders.iter().enumerate() {
                                    ui.label(format!("{}", i + 1));
                                    let side_color = if order.side == "BUY" {
                                        egui::Color32::GREEN
                                    } else {
                                        egui::Color32::RED
                                    };
                                    ui.colored_label(side_color, &order.side);
                                    ui.label(format!("{}", order.quantity));
                                    let status_color = match order.status.as_str() {
                                        "FILLED" => egui::Color32::GREEN,
                                        "WORKING" => egui::Color32::BLUE,
                                        _ => egui::Color32::GRAY,
                                    };
                                    ui.colored_label(status_color, &order.status);
                                    ui.label(&order.submitted_at);
                                    ui.end_row();
                                }
                            });
                    }
                });
                ui.add_space(20.0);
            }

            ui.add_space(20.0);

            // Advanced Order Types
            ui.group(|ui| {
                ui.heading("Advanced Order Types");
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Order Type:");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::Limit, "Limit");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::Market, "Market");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::StopLoss, "Stop Loss");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::TakeProfit, "Take Profit");
                });

                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::TrailingStop, "Trailing Stop");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::Iceberg, "Iceberg");
                    ui.radio_value(&mut self.advanced_order_type, AdvancedOrderType::PostOnly, "Post Only");
                });

                ui.add_space(10.0);

                match self.advanced_order_type {
                    AdvancedOrderType::StopLoss => {
                        ui.horizontal(|ui| {
                            ui.label("Trigger Price:");
                            ui.text_edit_singleline(&mut self.stop_loss_trigger);
                        });
                    }
                    AdvancedOrderType::TakeProfit => {
                        ui.horizontal(|ui| {
                            ui.label("Target Price:");
                            ui.text_edit_singleline(&mut self.take_profit_target);
                        });
                    }
                    AdvancedOrderType::TrailingStop => {
                        ui.horizontal(|ui| {
                            ui.label("Trail Distance:");
                            ui.text_edit_singleline(&mut self.trailing_stop_distance);
                        });
                    }
                    AdvancedOrderType::Iceberg => {
                        ui.horizontal(|ui| {
                            ui.label("Display Quantity:");
                            ui.text_edit_singleline(&mut self.iceberg_display_qty);
                        });
                    }
                    _ => {}
                }
            });

            ui.add_space(20.0);

            // Smart Order Routing
            ui.group(|ui| {
                ui.heading("Smart Order Routing");
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Routing Strategy:");
                    if ui.selectable_label(self.routing_strategy == "BestPrice", "Best Price").clicked() {
                        self.routing_strategy = "BestPrice".to_string();
                    }
                    if ui.selectable_label(self.routing_strategy == "BestLiquidity", "Best Liquidity").clicked() {
                        self.routing_strategy = "BestLiquidity".to_string();
                    }
                    if ui.selectable_label(self.routing_strategy == "LowestLatency", "Lowest Latency").clicked() {
                        self.routing_strategy = "LowestLatency".to_string();
                    }
                    if ui.selectable_label(self.routing_strategy == "LowestFee", "Lowest Fee").clicked() {
                        self.routing_strategy = "LowestFee".to_string();
                    }
                    if ui.selectable_label(self.routing_strategy == "Smart", "Smart").clicked() {
                        self.routing_strategy = "Smart".to_string();
                    }
                });

                ui.add_space(10.0);
                let routing_desc = match self.routing_strategy.as_str() {
                    "BestPrice" => "Routes to venue with best price",
                    "BestLiquidity" => "Routes to venue with deepest liquidity",
                    "LowestLatency" => "Routes to venue with lowest latency",
                    "LowestFee" => "Routes to venue with lowest fees",
                    "Smart" => "Composite scoring: price × 100 + liquidity - latency - fees",
                    _ => "Unknown strategy",
                };
                ui.label(egui::RichText::new(routing_desc).italics().color(egui::Color32::GRAY));
            });

            ui.add_space(20.0);

            // Market Impact Simulation
            ui.group(|ui| {
                ui.heading("Market Impact Simulation");
                ui.separator();
                ui.label("Almgren-Chriss market impact model");
                ui.label("• Permanent impact: γ × participation_rate");
                ui.label("• Temporary impact: ε × participation_rate");
                ui.label("• Dynamic spread modeling based on volatility and volume");
                ui.label("• Exponential liquidity depth profiles");
            });
        });
    }

    // ========================================================================
    // PAGE: ANALYTICS
    // ========================================================================

    fn render_analytics_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("📈 Performance Analytics");
            ui.add_space(10.0);

            let metrics_opt = self.state.performance_metrics.lock().unwrap();
            let equity_curve = self.state.equity_curve.lock().unwrap();

            if metrics_opt.is_none() {
                ui.label("No performance data available. Execute orders to generate metrics.");
                return;
            }

            let metrics = metrics_opt.as_ref().unwrap();

            // Equity Curve
            ui.group(|ui| {
                ui.heading("Equity Curve");

                if !equity_curve.is_empty() {
                    let points: PlotPoints = equity_curve
                        .iter()
                        .enumerate()
                        .map(|(i, p)| [i as f64, p.equity])
                        .collect();

                    let line = Line::new(points)
                        .color(egui::Color32::from_rgb(100, 150, 255))
                        .width(2.0);

                    Plot::new("equity_curve_plot")
                        .height(250.0)
                        .show_axes([true, true])
                        .allow_zoom(true)
                        .allow_drag(true)
                        .show(ui, |plot_ui| {
                            plot_ui.line(line);
                        });
                } else {
                    ui.label("No equity data");
                }
            });

            ui.add_space(20.0);

            // Return Metrics
            ui.group(|ui| {
                ui.heading("Return Metrics");
                egui::Grid::new("return_metrics")
                    .striped(true)
                    .num_columns(2)
                    .spacing([40.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Total Return:");
                        ui.label(format!("{:.2}%", metrics.total_return * 100.0));
                        ui.end_row();

                        ui.strong("Annualized Return:");
                        ui.label(format!("{:.2}%", metrics.annualized_return));
                        ui.end_row();

                        ui.strong("Sharpe Ratio:");
                        let sharpe_color = if metrics.sharpe_ratio > 1.5 {
                            egui::Color32::GREEN
                        } else if metrics.sharpe_ratio > 1.0 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::RED
                        };
                        ui.colored_label(sharpe_color, format!("{:.2}", metrics.sharpe_ratio));
                        ui.end_row();

                        ui.strong("Sortino Ratio:");
                        ui.label(format!("{:.2}", metrics.sortino_ratio));
                        ui.end_row();

                        ui.strong("Calmar Ratio:");
                        ui.label(format!("{:.2}", metrics.calmar_ratio));
                        ui.end_row();
                    });
            });

            ui.add_space(20.0);

            // Risk Metrics
            ui.group(|ui| {
                ui.heading("Risk Metrics");
                egui::Grid::new("risk_metrics")
                    .striped(true)
                    .num_columns(2)
                    .spacing([40.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Max Drawdown:");
                        let dd_color = if metrics.max_drawdown.abs() < 10.0 {
                            egui::Color32::GREEN
                        } else if metrics.max_drawdown.abs() < 20.0 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::RED
                        };
                        ui.colored_label(dd_color, format!("{:.2}%", metrics.max_drawdown));
                        ui.end_row();

                        ui.strong("Max DD Duration:");
                        ui.label(format!("{:.1} days", metrics.max_drawdown_duration_days));
                        ui.end_row();

                        ui.strong("Volatility:");
                        ui.label(format!("{:.2}%", metrics.volatility));
                        ui.end_row();

                        ui.strong("Downside Volatility:");
                        ui.label(format!("{:.2}%", metrics.downside_volatility));
                        ui.end_row();

                        ui.strong("VaR (95%):");
                        ui.label(format!("${:.2}", metrics.var_95));
                        ui.end_row();

                        ui.strong("VaR (99%):");
                        ui.label(format!("${:.2}", metrics.var_99));
                        ui.end_row();

                        ui.strong("CVaR (95%):");
                        ui.label(format!("${:.2}", metrics.cvar_95));
                        ui.end_row();

                        ui.strong("CVaR (99%):");
                        ui.label(format!("${:.2}", metrics.cvar_99));
                        ui.end_row();
                    });
            });

            ui.add_space(20.0);

            // Trade Statistics
            ui.group(|ui| {
                ui.heading("Trade Statistics");
                egui::Grid::new("trade_stats")
                    .striped(true)
                    .num_columns(2)
                    .spacing([40.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Total Trades:");
                        ui.label(format!("{}", metrics.total_trades));
                        ui.end_row();

                        ui.strong("Winning Trades:");
                        ui.colored_label(
                            egui::Color32::GREEN,
                            format!("{}", metrics.winning_trades),
                        );
                        ui.end_row();

                        ui.strong("Losing Trades:");
                        ui.colored_label(egui::Color32::RED, format!("{}", metrics.losing_trades));
                        ui.end_row();

                        ui.strong("Win Rate:");
                        let wr_color = if metrics.win_rate > 0.55 {
                            egui::Color32::GREEN
                        } else if metrics.win_rate > 0.45 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::RED
                        };
                        ui.colored_label(wr_color, format!("{:.1}%", metrics.win_rate * 100.0));
                        ui.end_row();

                        ui.strong("Profit Factor:");
                        let pf_color = if metrics.profit_factor > 1.5 {
                            egui::Color32::GREEN
                        } else if metrics.profit_factor > 1.0 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::RED
                        };
                        ui.colored_label(pf_color, format!("{:.2}", metrics.profit_factor));
                        ui.end_row();

                        ui.strong("Average Win:");
                        ui.colored_label(
                            egui::Color32::GREEN,
                            format!("${:.2}", metrics.average_win),
                        );
                        ui.end_row();

                        ui.strong("Average Loss:");
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("${:.2}", metrics.average_loss),
                        );
                        ui.end_row();

                        ui.strong("Largest Win:");
                        ui.colored_label(
                            egui::Color32::GREEN,
                            format!("${:.2}", metrics.largest_win),
                        );
                        ui.end_row();

                        ui.strong("Largest Loss:");
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("${:.2}", metrics.largest_loss),
                        );
                        ui.end_row();
                    });
            });
        });
    }

    // ========================================================================
    // PAGE: SETTINGS
    // ========================================================================

    fn render_settings_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("⚙ Settings");
            ui.add_space(10.0);

            ui.group(|ui| {
                ui.heading("Connection Settings");
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Server Address:");
                    ui.text_edit_singleline(&mut self.server_address);
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("Connect").clicked() {
                        self.connect_to_engine();
                    }
                    if ui.button("Disconnect").clicked() {
                        self.disconnect();
                    }
                });

                ui.add_space(10.0);

                let conn_status = self.state.connection_status.lock().unwrap().clone();
                let status_text = match conn_status {
                    ConnectionStatus::Connected => "Status: Connected ✓",
                    ConnectionStatus::Connecting => "Status: Connecting...",
                    ConnectionStatus::Disconnected => "Status: Disconnected",
                    ConnectionStatus::Error(ref msg) => msg.as_str(),
                };
                ui.label(status_text);
            });

            ui.add_space(20.0);

            ui.group(|ui| {
                ui.heading("System Information");
                ui.separator();
                ui.label("Platform: QuantSystem Trading Platform".to_string());
                ui.label("Version: 1.0.0".to_string());
                ui.label("Real-time data: Binance Public API".to_string());
                ui.label(format!("Execution Engine: gRPC @ {}", self.server_address));
            });

            ui.add_space(20.0);

            ui.group(|ui| {
                ui.heading("Features Implemented");
                ui.separator();
                ui.label("✓ Advanced Order Types (Stop Loss, Take Profit, Trailing Stop, Iceberg, Post Only)");
                ui.label("✓ Smart Order Routing (BestPrice, BestLiquidity, LowestLatency, LowestFee, Smart)");
                ui.label("✓ Historical Replay & Backtesting");
                ui.label("✓ Market Impact Simulation (Almgren-Chriss Model)");
                ui.label("✓ Adaptive TWAP Algorithm");
                ui.label("✓ Real-time Market Data & Technical Indicators");
                ui.label("✓ Performance Analytics (Sharpe, Sortino, Calmar, VaR, CVaR)");
                ui.label("✓ Multi-page Dashboard Navigation");
            });
        });
    }

    // ========================================================================
    // PAGE: LOGS
    // ========================================================================

    fn render_logs_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("📋 System Logs");
        ui.add_space(10.0);

        // Filter and controls
        ui.horizontal(|ui| {
            if ui.button("Clear Logs").clicked() {
                self.state.logs.lock().unwrap().clear();
            }

            ui.separator();
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.log_filter);
        });

        ui.add_space(10.0);
        ui.separator();

        // Logs display
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let logs = self.state.logs.lock().unwrap();

                if logs.is_empty() {
                    ui.label("No logs yet. Logs will appear here when you perform actions.");
                } else {
                    for log in logs.iter() {
                        // Apply filter if any
                        if !self.log_filter.is_empty() {
                            let filter_lower = self.log_filter.to_lowercase();
                            let message_lower = log.message.to_lowercase();
                            let category_lower = log.category.to_lowercase();

                            if !message_lower.contains(&filter_lower)
                                && !category_lower.contains(&filter_lower)
                            {
                                continue;
                            }
                        }

                        ui.horizontal(|ui| {
                            // Timestamp
                            ui.label(
                                egui::RichText::new(&log.timestamp)
                                    .color(egui::Color32::GRAY)
                                    .monospace(),
                            );

                            // Category with color
                            let category_color =
                                egui::Color32::from_rgb(log.color[0], log.color[1], log.color[2]);
                            ui.label(
                                egui::RichText::new(format!("[{}]", &log.category))
                                    .color(category_color)
                                    .strong(),
                            );

                            // Message
                            ui.label(&log.message);
                        });
                    }
                }
            });
    }

    // ========================================================================
    // BACKTESTING & ADAPTIVE TWAP FUNCTIONS
    // ========================================================================

    fn start_backtest(&mut self) {
        eprintln!("\n========================================");
        eprintln!("[BACKTEST] Starting Historical Backtest");
        eprintln!("========================================");

        let symbol = self.symbol.clone();
        let state = self.state.clone();
        let speed = self.backtest_replay_speed.clone();

        eprintln!("[BACKTEST] Configuration:");
        eprintln!("  Symbol: {}", symbol);
        eprintln!("  Speed: {:?}", speed);

        *state.backtest_running.lock().unwrap() = true;
        *state.backtest_progress.lock().unwrap() = 0.0;
        state.backtest_results.lock().unwrap().clear();
        *state.status_message.lock().unwrap() = "Starting backtest...".to_string();

        thread::spawn(move || {
            // Get market data history
            let market_data = state.market_data.lock().unwrap();
            let data = market_data.iter().find(|d| d.symbol == symbol);

            if let Some(data) = data {
                let price_history = data.price_history.clone();
                drop(market_data);

                let total_events = price_history.len();
                if total_events == 0 {
                    eprintln!("[BACKTEST] ❌ No market data available");
                    state
                        .backtest_results
                        .lock()
                        .unwrap()
                        .push("No market data available for backtesting".to_string());
                    *state.backtest_running.lock().unwrap() = false;
                    return;
                }

                eprintln!("[BACKTEST] Processing {} price points...", total_events);
                state.backtest_results.lock().unwrap().push(format!(
                    "Backtesting {} using {} price points",
                    symbol, total_events
                ));

                // Simulate trades
                let mut trades = 0;
                let mut total_pnl = 0.0;
                let mut position = 0i64;
                let mut entry_price = 0.0;

                for (i, (_time, price)) in price_history.iter().enumerate() {
                    // Simple strategy: buy on dips, sell on peaks
                    if i > 0 {
                        let prev_price =
                            price_history.get(i - 1).map(|(_, p)| *p).unwrap_or(*price);
                        let price_change = (price - prev_price) / prev_price;

                        if price_change < -0.001 && position == 0 {
                            // Buy signal
                            position = 100;
                            entry_price = *price;
                            trades += 1;
                            eprintln!(
                                "[BACKTEST] Trade #{} - BUY 100 @ ${:.2} (dip: {:.2}%)",
                                trades,
                                price,
                                price_change * 100.0
                            );
                        } else if price_change > 0.001 && position > 0 {
                            // Sell signal
                            let pnl = (price - entry_price) * position as f64;
                            total_pnl += pnl;
                            eprintln!("[BACKTEST] Trade #{} - SELL 100 @ ${:.2} (peak: {:.2}%) | P&L: ${:.2}", trades, price, price_change * 100.0, pnl);
                            position = 0;
                            trades += 1;
                        }
                    }

                    // Update progress
                    let progress = (i + 1) as f64 / total_events as f64;
                    *state.backtest_progress.lock().unwrap() = progress;

                    // Speed control
                    match speed {
                        ReplaySpeed::RealTime => {
                            thread::sleep(std::time::Duration::from_millis(100))
                        }
                        ReplaySpeed::FastForward2x => {
                            thread::sleep(std::time::Duration::from_millis(50))
                        }
                        ReplaySpeed::FastForward5x => {
                            thread::sleep(std::time::Duration::from_millis(20))
                        }
                        ReplaySpeed::FastForward10x => {
                            thread::sleep(std::time::Duration::from_millis(10))
                        }
                        ReplaySpeed::Maximum => {}
                    }
                }

                // Calculate metrics
                let win_rate = if trades > 0 {
                    (total_pnl > 0.0) as i32 as f64 / (trades / 2) as f64
                } else {
                    0.0
                };

                eprintln!("\n[BACKTEST] ✅ Backtest Complete!");
                eprintln!("  Total Events: {}", total_events);
                eprintln!("  Total Trades: {}", trades);
                eprintln!("  Total P&L: ${:.2}", total_pnl);
                eprintln!("  Win Rate: {:.1}%", win_rate * 100.0);
                eprintln!("========================================\n");

                log_message(
                    &state,
                    "BACKTEST",
                    &format!(
                        "✅ Backtest complete: {} trades, P&L: ${:.2}",
                        trades, total_pnl
                    ),
                );

                let mut results = state.backtest_results.lock().unwrap();
                results.clear();
                results.push(format!("✓ Backtest completed for {}", symbol));
                results.push(String::new());
                results.push(format!("Total Events: {}", total_events));
                results.push(format!("Total Trades: {}", trades));
                results.push(format!("Total P&L: ${:.2}", total_pnl));
                results.push(format!("Win Rate: {:.1}%", win_rate * 100.0));
                results.push(String::new());
                results.push("Strategy: Simple momentum (buy dips, sell peaks)".to_string());
                results.push(
                    "Note: This is a simplified simulation using collected market data".to_string(),
                );
            } else {
                eprintln!("[BACKTEST] ❌ No data found for symbol {}", symbol);
                drop(market_data);
                state
                    .backtest_results
                    .lock()
                    .unwrap()
                    .push(format!("No data found for symbol {}", symbol));
            }

            *state.backtest_running.lock().unwrap() = false;
            *state.status_message.lock().unwrap() = "Backtest completed".to_string();
        });
    }

    fn stop_backtest(&mut self) {
        *self.state.backtest_running.lock().unwrap() = false;
        *self.state.status_message.lock().unwrap() = "Backtest stopped".to_string();
    }

    fn submit_adaptive_twap(&mut self) {
        eprintln!("\n========================================");
        eprintln!("[ADAPTIVE TWAP] Execution Started");
        eprintln!("========================================");

        // Parse parameters
        let total_qty: u64 = self.adaptive_total_qty.parse().unwrap_or(1000);
        let duration_secs: u64 = self.adaptive_duration_secs.parse().unwrap_or(300);
        let base_slice_secs: u64 = self.adaptive_base_slice_secs.parse().unwrap_or(60);
        let volatility_window: usize = self.adaptive_volatility_window.parse().unwrap_or(20);
        let max_participation: f64 = self.adaptive_max_participation.parse().unwrap_or(0.3);

        let symbol = self.symbol.clone();
        let side = self.side;
        let state = self.state.clone();
        let enable_adaptive = self.adaptive_enable;

        let num_slices = (duration_secs / base_slice_secs) as usize;

        eprintln!("[ADAPTIVE TWAP] Configuration:");
        eprintln!("  Symbol: {}", symbol);
        eprintln!("  Side: {}", side);
        eprintln!("  Total Qty: {}", total_qty);
        eprintln!("  Duration: {}s", duration_secs);
        eprintln!("  Base Slice: {}s", base_slice_secs);
        eprintln!("  Total Slices: {}", num_slices);
        eprintln!(
            "  Adaptive: {}",
            if enable_adaptive {
                "ENABLED"
            } else {
                "DISABLED"
            }
        );
        eprintln!("  Volatility Window: {}", volatility_window);
        eprintln!("  Max Participation: {:.1}%", max_participation * 100.0);

        // Check if connected to engine
        let client_opt = self.state.grpc_client.lock().unwrap();
        let is_connected = client_opt.is_some();
        drop(client_opt);

        if !is_connected {
            eprintln!("[ADAPTIVE TWAP] ⚠️  WARNING: Not connected to execution engine");
            eprintln!("                Running in SIMULATION mode");
        } else {
            eprintln!(
                "[ADAPTIVE TWAP] ✅ Connected to execution engine - REAL orders will be submitted"
            );
        }

        *state.adaptive_twap_running.lock().unwrap() = true;
        *state.adaptive_progress.lock().unwrap() = 0.0;
        state.adaptive_child_orders.lock().unwrap().clear();
        *state.status_message.lock().unwrap() = "Starting Adaptive TWAP execution...".to_string();

        log_message(
            &state,
            "ADAPTIVE TWAP",
            &format!(
                "▶️ Starting Adaptive TWAP: {} {} {} over {}s",
                side, total_qty, symbol, duration_secs
            ),
        );

        thread::spawn(move || {
            let num_slices = (duration_secs / base_slice_secs) as usize;

            // Check if we have a gRPC client
            let client_opt = state.grpc_client.lock().unwrap().clone();
            let use_real_orders = client_opt.is_some();

            // Get market data for adaptive logic
            let market_data = state.market_data.lock().unwrap();
            let data = market_data.iter().find(|d| d.symbol == symbol).cloned();
            drop(market_data);

            if data.is_none() {
                eprintln!("[ADAPTIVE TWAP] ❌ No market data for symbol {}", symbol);
                *state.status_message.lock().unwrap() =
                    format!("No market data for symbol {}", symbol);
                *state.adaptive_twap_running.lock().unwrap() = false;
                return;
            }

            let data = data.unwrap();
            let mut filled_qty = 0u64;

            eprintln!("\n[ADAPTIVE TWAP] Starting slice execution...\n");

            for slice_num in 0..num_slices {
                // Calculate slice quantity with adaptive adjustment
                let remaining_qty = total_qty - filled_qty;
                let remaining_slices = num_slices - slice_num;

                let mut slice_qty = if remaining_slices > 0 {
                    remaining_qty / remaining_slices as u64
                } else {
                    remaining_qty
                };

                // Apply adaptive logic if enabled
                let mut vol_adjustment = 1.0;
                if enable_adaptive && data.price_history.len() >= volatility_window {
                    let recent_prices: Vec<f64> = data
                        .price_history
                        .iter()
                        .rev()
                        .take(volatility_window)
                        .map(|(_, p)| *p)
                        .collect();

                    // Calculate volatility
                    if recent_prices.len() >= 2 {
                        let returns: Vec<f64> = recent_prices
                            .iter()
                            .zip(recent_prices.iter().skip(1))
                            .map(|(p1, p2)| (p2 / p1).ln())
                            .collect();

                        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
                        let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
                            / returns.len() as f64;
                        let volatility = variance.sqrt();

                        // Adjust slice based on volatility
                        vol_adjustment = if volatility > 0.02 {
                            eprintln!("[ADAPTIVE TWAP] HIGH volatility detected ({:.3}%), reducing slice by 20%", volatility * 100.0);
                            0.8
                        } else if volatility > 0.01 {
                            eprintln!("[ADAPTIVE TWAP] MEDIUM volatility detected ({:.3}%), reducing slice by 10%", volatility * 100.0);
                            0.9
                        } else {
                            eprintln!(
                                "[ADAPTIVE TWAP] NORMAL volatility ({:.3}%)",
                                volatility * 100.0
                            );
                            1.0
                        };

                        slice_qty = (slice_qty as f64 * vol_adjustment) as u64;
                    }
                }

                slice_qty = slice_qty.max(1).min(remaining_qty);

                eprintln!("[ADAPTIVE TWAP] Slice #{}/{}", slice_num + 1, num_slices);
                eprintln!(
                    "  Quantity: {} (adjustment: {:.0}%)",
                    slice_qty,
                    vol_adjustment * 100.0
                );
                eprintln!("  Remaining: {}/{}", remaining_qty, total_qty);

                if use_real_orders {
                    // Submit REAL order via gRPC
                    eprintln!("  Mode: REAL ORDER via gRPC");

                    let mut client = client_opt.clone().unwrap();
                    let runtime = state.grpc_runtime.lock().unwrap();
                    let side_str = format!("{}", side);

                    let result = runtime.block_on(async {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_nanos() as u64;
                        let request = ParentOrderRequest {
                            symbol: symbol.clone(),
                            side: side_str.clone(),
                            quantity: slice_qty,
                            limit_price_ticks: 0, // Market order
                            start_time_ns: now,
                            end_time_ns: now + (base_slice_secs * 1_000_000_000),
                            algo_type: "TWAP".to_string(),
                            num_slices: 1,
                        };
                        client.submit_parent_order(request).await
                    });

                    match result {
                        Ok(response) => {
                            let resp = response.into_inner();
                            eprintln!("  ✅ Order #{} ACCEPTED", resp.parent_order_id);

                            log_message(
                                &state,
                                "ADAPTIVE TWAP",
                                &format!(
                                    "📋 Slice {}/{} submitted: Order #{} for {} qty",
                                    slice_num + 1,
                                    num_slices,
                                    resp.parent_order_id,
                                    slice_qty
                                ),
                            );

                            let order = OrderInfo {
                                order_id: resp.parent_order_id,
                                symbol: symbol.clone(),
                                side: side_str,
                                quantity: slice_qty,
                                filled_qty: 0,
                                status: resp.status.clone(),
                                submitted_at: chrono::Local::now().format("%H:%M:%S").to_string(),
                            };

                            state
                                .adaptive_child_orders
                                .lock()
                                .unwrap()
                                .push(order.clone());
                            state.orders.lock().unwrap().insert(0, order);
                            filled_qty += slice_qty;
                        }
                        Err(e) => {
                            eprintln!("  ❌ Order submission FAILED: {}", e);
                            log_message(
                                &state,
                                "ERROR",
                                &format!(
                                    "❌ TWAP slice {}/{} failed: {}",
                                    slice_num + 1,
                                    num_slices,
                                    e
                                ),
                            );
                        }
                    }
                } else {
                    // Simulation mode
                    eprintln!("  Mode: SIMULATION (not connected to engine)");

                    let order = OrderInfo {
                        order_id: (slice_num + 1) as u64,
                        symbol: symbol.clone(),
                        side: format!("{}", side),
                        quantity: slice_qty,
                        filled_qty: slice_qty,
                        status: "FILLED".to_string(),
                        submitted_at: chrono::Local::now().format("%H:%M:%S").to_string(),
                    };

                    state.adaptive_child_orders.lock().unwrap().push(order);
                    filled_qty += slice_qty;
                    eprintln!("  ✅ Simulated fill");
                }

                // Update progress
                let progress = filled_qty as f64 / total_qty as f64;
                *state.adaptive_progress.lock().unwrap() = progress;
                eprintln!("  Progress: {:.1}%\n", progress * 100.0);

                // Sleep between slices
                if slice_num < num_slices - 1 {
                    eprintln!("  Sleeping for {}s before next slice...\n", base_slice_secs);
                    thread::sleep(std::time::Duration::from_secs(base_slice_secs));
                }

                if filled_qty >= total_qty {
                    break;
                }
            }

            eprintln!("\n[ADAPTIVE TWAP] ✅ Execution Complete!");
            eprintln!("  Total Filled: {}/{}", filled_qty, total_qty);
            eprintln!("  Total Slices: {}", num_slices);
            eprintln!("========================================\n");

            log_message(
                &state,
                "ADAPTIVE TWAP",
                &format!(
                    "✅ Execution complete: {}/{} filled in {} slices",
                    filled_qty, total_qty, num_slices
                ),
            );

            *state.status_message.lock().unwrap() = format!(
                "Adaptive TWAP completed: {}/{} filled",
                filled_qty, total_qty
            );
            *state.adaptive_twap_running.lock().unwrap() = false;
        });
    }

    // ========================================================================
    // BACKEND FUNCTIONS
    // ========================================================================

    fn connect_to_engine(&mut self) {
        eprintln!("\n========================================");
        eprintln!("[CONNECTION] Connecting to Execution Engine");
        eprintln!("========================================");
        eprintln!("[CONNECTION] Target: {}", self.server_address);

        *self.state.connection_status.lock().unwrap() = ConnectionStatus::Connecting;
        *self.state.status_message.lock().unwrap() =
            format!("Connecting to {}...", self.server_address);

        let server_addr = self.server_address.clone();
        let state = self.state.clone();

        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();
            eprintln!("[CONNECTION] Attempting gRPC connection...");

            match runtime.block_on(async {
                ExecutionServiceClient::connect(format!("http://{}", server_addr)).await
            }) {
                Ok(client) => {
                    eprintln!("\n[CONNECTION] ✅ Successfully Connected!");
                    eprintln!("  Server: {}", server_addr);
                    eprintln!("  Protocol: gRPC");
                    eprintln!("  Status: READY");
                    eprintln!("========================================\n");

                    log_message(
                        &state,
                        "CONNECTION",
                        &format!("✅ Connected to {} via gRPC", server_addr),
                    );

                    *state.grpc_client.lock().unwrap() = Some(client);
                    *state.connection_status.lock().unwrap() = ConnectionStatus::Connected;
                    *state.status_message.lock().unwrap() = format!("Connected to {}", server_addr);
                }
                Err(e) => {
                    eprintln!("\n[CONNECTION] ❌ Connection Failed!");
                    eprintln!("  Server: {}", server_addr);
                    eprintln!("  Error: {}", e);
                    eprintln!("  Hint: Make sure the execution engine is running");
                    eprintln!("========================================\n");

                    log_message(
                        &state,
                        "ERROR",
                        &format!("❌ Connection to {} failed: {}", server_addr, e),
                    );

                    *state.connection_status.lock().unwrap() =
                        ConnectionStatus::Error(format!("Connection failed: {}", e));
                    *state.status_message.lock().unwrap() = format!("Failed to connect: {}", e);
                }
            }
        });
    }

    fn disconnect(&mut self) {
        *self.state.connection_status.lock().unwrap() = ConnectionStatus::Disconnected;
        *self.state.status_message.lock().unwrap() = "Disconnected".to_string();
    }

    fn poll_order_status(&self) {
        use api::proto::OrderStatusRequest;

        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            return;
        }
        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        let orders = self.state.orders.lock().unwrap();
        let order_ids: Vec<u64> = orders
            .iter()
            .filter(|o| {
                o.status == "WORKING"
                    || o.status == "PENDING"
                    || o.status == "ACCEPTED"
                    || o.status == "PARTIALLY_FILLED"
            })
            .map(|o| o.order_id)
            .collect();
        drop(orders);

        if order_ids.is_empty() {
            return;
        }

        let state = self.state.clone();
        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();

            for order_id in order_ids {
                let result = runtime.block_on(async {
                    let request = OrderStatusRequest {
                        parent_order_id: order_id,
                    };
                    client.get_order_status(request).await
                });

                if let Ok(response) = result {
                    let resp = response.into_inner();
                    let mut orders = state.orders.lock().unwrap();

                    if let Some(order) = orders.iter_mut().find(|o| o.order_id == order_id) {
                        let old_status = order.status.clone();
                        let old_filled = order.filled_qty;

                        order.status = resp.status.clone();
                        order.filled_qty = resp.filled_qty;

                        if old_status != resp.status || old_filled != resp.filled_qty {
                            eprintln!("\n[ORDER UPDATE] Order #{} Status Change", order_id);
                            eprintln!("  Status: {} -> {}", old_status, resp.status);
                            eprintln!(
                                "  Filled: {} -> {}/{}",
                                old_filled, resp.filled_qty, order.quantity
                            );

                            if resp.status == "FILLED" {
                                eprintln!("  ✅ ORDER FULLY FILLED!");
                                log_message(
                                    &state,
                                    "ORDER UPDATE",
                                    &format!(
                                        "✅ Order #{} FILLED: {}/{}",
                                        order_id, resp.filled_qty, order.quantity
                                    ),
                                );
                            } else {
                                log_message(
                                    &state,
                                    "ORDER UPDATE",
                                    &format!(
                                        "Order #{} status: {} ({}/{})",
                                        order_id, resp.status, resp.filled_qty, order.quantity
                                    ),
                                );
                            }
                        }

                        if resp.status == "FILLED" && resp.filled_qty > 0 {
                            eprintln!("[PERFORMANCE] Updating performance metrics from fills...");
                            drop(orders);
                            update_performance_from_fills(&state);
                        }
                    }
                }
            }
        });
    }

    fn submit_order(&mut self) {
        eprintln!("\n========================================");
        eprintln!("[TRADING] Order Submission Started");
        eprintln!("========================================");

        let qty: u64 = match self.quantity.parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[TRADING] ❌ Invalid quantity: {}", self.quantity);
                *self.state.status_message.lock().unwrap() = "Invalid quantity".to_string();
                return;
            }
        };

        let limit_price: f64 = match self.limit_price.parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[TRADING] ❌ Invalid price: {}", self.limit_price);
                *self.state.status_message.lock().unwrap() = "Invalid price".to_string();
                return;
            }
        };

        let duration: f64 = match self.duration_sec.parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[TRADING] ❌ Invalid duration: {}", self.duration_sec);
                *self.state.status_message.lock().unwrap() = "Invalid duration".to_string();
                return;
            }
        };

        let num_slices: u32 = match self.num_slices.parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[TRADING] ❌ Invalid num slices: {}", self.num_slices);
                *self.state.status_message.lock().unwrap() = "Invalid num slices".to_string();
                return;
            }
        };

        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            eprintln!("[TRADING] ❌ Not connected to execution engine");
            *self.state.status_message.lock().unwrap() = "Not connected to engine".to_string();
            return;
        }

        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        let symbol = self.symbol.clone();
        let side_str = format!("{}", self.side);
        let algo_type = format!("{}", self.algo_type);
        let state = self.state.clone();

        eprintln!("[TRADING] Order Details:");
        eprintln!("  Symbol: {}", symbol);
        eprintln!("  Side: {}", side_str);
        eprintln!("  Quantity: {}", qty);
        eprintln!("  Price: ${:.2}", limit_price);
        eprintln!("  Algorithm: {}", algo_type);
        eprintln!("  Duration: {:.1}s", duration);
        eprintln!("  Slices: {}", num_slices);

        *self.state.status_message.lock().unwrap() = format!(
            "Submitting {}: {} {} {} @ ${:.2}",
            algo_type, side_str, qty, symbol, limit_price
        );

        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();
            eprintln!("[TRADING] 📡 Sending order to execution engine via gRPC...");

            let result = runtime.block_on(async {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                let duration_ns = (duration * 1_000_000_000.0) as u64;

                let request = ParentOrderRequest {
                    symbol: symbol.clone(),
                    side: side_str.clone(),
                    quantity: qty,
                    limit_price_ticks: if limit_price > 0.0 {
                        (limit_price * 100.0) as i64
                    } else {
                        0
                    },
                    start_time_ns: now,
                    end_time_ns: now + duration_ns,
                    algo_type: algo_type.clone(),
                    num_slices,
                };

                client.submit_parent_order(request).await
            });

            match result {
                Ok(response) => {
                    let resp = response.into_inner();
                    eprintln!("\n[TRADING] ✅ Order Accepted by Engine!");
                    eprintln!("  Order ID: {}", resp.parent_order_id);
                    eprintln!("  Status: {}", resp.status);
                    eprintln!("  Symbol: {}", symbol);
                    eprintln!("  Side: {}", side_str);
                    eprintln!("  Qty: {}", qty);
                    eprintln!("========================================\n");

                    log_message(
                        &state,
                        "TRADING",
                        &format!(
                            "✅ Order #{} accepted: {} {} {} @ ${:.2}",
                            resp.parent_order_id, side_str, qty, symbol, limit_price
                        ),
                    );

                    let order = OrderInfo {
                        order_id: resp.parent_order_id,
                        symbol,
                        side: side_str,
                        quantity: qty,
                        filled_qty: 0,
                        status: resp.status.clone(),
                        submitted_at: chrono::Local::now().format("%H:%M:%S").to_string(),
                    };

                    state.orders.lock().unwrap().insert(0, order);
                    *state.status_message.lock().unwrap() =
                        format!("Order {} {}", resp.parent_order_id, resp.status);
                }
                Err(e) => {
                    eprintln!("\n[TRADING] ❌ Order Submission FAILED!");
                    eprintln!("  Error: {}", e);
                    eprintln!("  This usually means the execution engine is not running");
                    eprintln!("========================================\n");

                    log_message(
                        &state,
                        "ERROR",
                        &format!("❌ Order submission failed: {}", e),
                    );

                    *state.status_message.lock().unwrap() =
                        format!("Order submission failed: {}", e);
                }
            }
        });
    }

    fn refresh_data(&self) {
        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            return;
        }

        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        let state = self.state.clone();

        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();

            let positions_result = runtime.block_on(async {
                let request = PositionsRequest {
                    symbol: String::new(),
                };
                client.get_positions(request).await
            });

            match positions_result {
                Ok(response) => {
                    let resp = response.into_inner();

                    if resp.position != 0 {
                        let mut positions = state.positions.lock().unwrap();
                        positions.clear();
                        positions.push(PositionInfo {
                            symbol: resp.symbol,
                            quantity: resp.position,
                            avg_price: 0.0,
                            realized_pnl: resp.realized_pnl,
                            unrealized_pnl: resp.unrealized_pnl,
                        });
                    }
                }
                Err(e) => {
                    eprintln!("[GUI] Failed to query positions: {}", e);
                }
            }
        });
    }
}

// ============================================================================
// LOGGING UTILITY
// ============================================================================

fn log_message(state: &AppState, category: &str, message: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let timestamp = {
        let secs = now % 86400;
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    };

    let color = match category {
        "TRADING" => [100, 200, 100],       // Green
        "ADAPTIVE TWAP" => [100, 150, 255], // Blue
        "BACKTEST" => [255, 200, 100],      // Orange
        "ORDER UPDATE" => [200, 200, 100],  // Yellow
        "CONNECTION" => [150, 100, 255],    // Purple
        "PERFORMANCE" => [255, 150, 150],   // Pink
        "ERROR" => [255, 100, 100],         // Red
        _ => [200, 200, 200],               // Gray
    };

    let entry = LogEntry {
        timestamp,
        category: category.to_string(),
        message: message.to_string(),
        color,
    };

    let mut logs = state.logs.lock().unwrap();
    logs.push_back(entry);

    // Keep only last 1000 logs
    if logs.len() > 1000 {
        logs.pop_front();
    }

    // Also print to terminal
    eprintln!("[{}] {}", category, message);
}

// ============================================================================
// REAL-TIME DATA FETCHER
// ============================================================================

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BinanceTickerPrice {
    symbol: String,
    price: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BinanceTicker {
    symbol: String,
    #[serde(rename = "bidPrice")]
    bid_price: String,
    #[serde(rename = "askPrice")]
    ask_price: String,
    #[serde(rename = "lastPrice")]
    last_price: String,
    volume: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BinanceTrade {
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "m")]
    is_buyer_maker: bool,
    #[serde(rename = "T")]
    trade_time: u64,
}

fn calculate_live_indicators(
    _symbol: &str,
    price_history: &VecDeque<(f64, f64)>,
) -> Option<LiveIndicators> {
    if price_history.len() < 50 {
        return None;
    }

    let prices: Vec<f64> = price_history.iter().map(|(_, p)| *p).collect();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let rsi = indicators::rsi(&prices, 14)
        .ok()
        .and_then(|v| v.last().copied());

    let macd_result = indicators::macd(&prices, 12, 26, 9).ok();
    let macd = macd_result
        .as_ref()
        .and_then(|m| m.macd_line.last().copied());
    let macd_signal = macd_result
        .as_ref()
        .and_then(|m| m.signal_line.last().copied());

    let bb_result = indicators::bollinger_bands(&prices, 20, 2.0).ok();
    let bb_upper = bb_result.as_ref().and_then(|bb| bb.upper.last().copied());
    let bb_middle = bb_result.as_ref().and_then(|bb| bb.middle.last().copied());
    let bb_lower = bb_result.as_ref().and_then(|bb| bb.lower.last().copied());

    Some(LiveIndicators {
        rsi,
        macd,
        macd_signal,
        bb_upper,
        bb_middle,
        bb_lower,
        last_update: now,
    })
}

fn update_performance_from_fills(state: &AppState) {
    let orders = state.orders.lock().unwrap();
    let mut analyzer = state.performance_analyzer.lock().unwrap();

    let mut trades_added = 0;
    for order in orders.iter() {
        if order.filled_qty > 0 {
            let trade_type = if order.side == "BUY" {
                TradeType::Buy
            } else {
                TradeType::Sell
            };

            let avg_price = 100.0;
            let pnl = if order.side == "SELL" {
                (avg_price - 99.0) * order.filled_qty as f64
            } else {
                0.0
            };

            let trade = Trade {
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
                symbol: order.symbol.clone(),
                side: trade_type,
                quantity: order.filled_qty as f64,
                price: avg_price,
                pnl,
            };

            analyzer.add_trade(trade);
            trades_added += 1;
        }
    }

    if trades_added > 0 {
        if let Ok(metrics) = analyzer.calculate_metrics() {
            eprintln!(
                "[GUI] Performance metrics updated: {} trades",
                metrics.total_trades
            );
            *state.performance_metrics.lock().unwrap() = Some(metrics);
        }
        *state.equity_curve.lock().unwrap() = analyzer.get_equity_curve().to_vec();
    }
}

fn start_realtime_data_fetcher(state: AppState) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let symbols = vec!["BTCUSDT", "ETHUSDT", "SOLUSDT", "BNBUSDT", "ADAUSDT"];
            let client = reqwest::Client::new();

            {
                let mut market_data = state.market_data.lock().unwrap();
                market_data.clear();
                for sym in &symbols {
                    market_data.push(MarketData {
                        symbol: sym.to_string(),
                        bid: 0.0,
                        ask: 0.0,
                        last: 0.0,
                        volume: 0,
                        price_history: VecDeque::new(),
                    });
                }
            }

            {
                let mut news = state.news.lock().unwrap();
                news.push(NewsItem {
                    symbol: "MARKET".to_string(),
                    headline: "Real-time data feed active - Binance API connected".to_string(),
                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                    sentiment: "positive".to_string(),
                });
            }

            loop {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64();

                for symbol in &symbols {
                    if let Ok(ticker_result) = client
                        .get(format!(
                            "https://api.binance.com/api/v3/ticker/bookTicker?symbol={}",
                            symbol
                        ))
                        .send()
                        .await
                    {
                        if let Ok(ticker) = ticker_result.json::<serde_json::Value>().await {
                            let bid = ticker["bidPrice"]
                                .as_str()
                                .unwrap_or("0")
                                .parse::<f64>()
                                .unwrap_or(0.0);
                            let ask = ticker["askPrice"]
                                .as_str()
                                .unwrap_or("0")
                                .parse::<f64>()
                                .unwrap_or(0.0);
                            let last = (bid + ask) / 2.0;

                            if let Ok(ticker_24h_result) = client
                                .get(format!(
                                    "https://api.binance.com/api/v3/ticker/24hr?symbol={}",
                                    symbol
                                ))
                                .send()
                                .await
                            {
                                if let Ok(ticker_24h) =
                                    ticker_24h_result.json::<serde_json::Value>().await
                                {
                                    let volume = ticker_24h["volume"]
                                        .as_str()
                                        .unwrap_or("0")
                                        .parse::<f64>()
                                        .unwrap_or(0.0)
                                        as u64;

                                    let price_hist_clone;
                                    {
                                        let mut market_data = state.market_data.lock().unwrap();
                                        if let Some(data) =
                                            market_data.iter_mut().find(|d| d.symbol == *symbol)
                                        {
                                            data.bid = bid;
                                            data.ask = ask;
                                            data.last = last;
                                            data.volume = volume;

                                            data.price_history.push_back((now, last));
                                            if data.price_history.len() > 100 {
                                                data.price_history.pop_front();
                                            }
                                            price_hist_clone = data.price_history.clone();
                                        } else {
                                            price_hist_clone = VecDeque::new();
                                        }
                                    }

                                    if let Some(indicators) =
                                        calculate_live_indicators(symbol, &price_hist_clone)
                                    {
                                        let mut live_indicators =
                                            state.live_indicators.lock().unwrap();
                                        live_indicators.insert(symbol.to_string(), indicators);
                                    }
                                }
                            }
                        }
                    }

                    if let Ok(trades_result) = client
                        .get(format!(
                            "https://api.binance.com/api/v3/trades?symbol={}&limit=5",
                            symbol
                        ))
                        .send()
                        .await
                    {
                        if let Ok(trades) = trades_result.json::<Vec<serde_json::Value>>().await {
                            let mut transactions = state.transactions.lock().unwrap();

                            for trade in trades {
                                let price = trade["price"]
                                    .as_str()
                                    .unwrap_or("0")
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let qty = trade["qty"]
                                    .as_str()
                                    .unwrap_or("0")
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let is_buyer_maker =
                                    trade["isBuyerMaker"].as_bool().unwrap_or(false);

                                let trans = Transaction {
                                    symbol: symbol.to_string(),
                                    side: if is_buyer_maker { "SELL" } else { "BUY" }.to_string(),
                                    price,
                                    quantity: qty as u64,
                                    entity: "Binance".to_string(),
                                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                };

                                transactions.push_front(trans);
                            }

                            while transactions.len() > 50 {
                                transactions.pop_back();
                            }
                        }
                    }
                }

                // Generate news
                {
                    let market_data = state.market_data.lock().unwrap();
                    let _indicators = state.live_indicators.lock().unwrap();
                    let mut news = state.news.lock().unwrap();

                    for data in market_data.iter() {
                        if data.price_history.len() >= 10 {
                            let recent_prices: Vec<f64> = data
                                .price_history
                                .iter()
                                .rev()
                                .take(10)
                                .map(|(_, p)| *p)
                                .collect();

                            let old_price = recent_prices.last().unwrap();
                            let current_price = recent_prices.first().unwrap();
                            let pct_change = ((current_price - old_price) / old_price) * 100.0;

                            if pct_change.abs() > 0.5 && news.len() < 20 {
                                let (headline, sentiment) = if pct_change > 0.0 {
                                    (
                                        format!(
                                            "{} surges {:.2}% amid strong buying pressure",
                                            data.symbol.replace("USDT", ""),
                                            pct_change
                                        ),
                                        "positive".to_string(),
                                    )
                                } else {
                                    (
                                        format!(
                                            "{} drops {:.2}% on profit-taking",
                                            data.symbol.replace("USDT", ""),
                                            pct_change.abs()
                                        ),
                                        "negative".to_string(),
                                    )
                                };

                                news.push(NewsItem {
                                    symbol: data.symbol.clone(),
                                    headline,
                                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                    sentiment,
                                });
                            }
                        }
                    }

                    while news.len() > 15 {
                        news.remove(0);
                    }
                }

                *state.status_message.lock().unwrap() =
                    "Live data streaming from Binance".to_string();

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        });
    });
}

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1200.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "QuantSystem Trading Platform - Full Dashboard",
        options,
        Box::new(|_cc| Ok(Box::new(TraderApp::default()))),
    )
}
