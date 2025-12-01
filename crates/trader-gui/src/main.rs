use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use orderbook::Side;
use serde::Deserialize;
use analytics::{PerformanceMetrics, EquityPoint, Trade, TradeType, PerformanceAnalyzer};
use api::proto::execution_service_client::ExecutionServiceClient;
use api::proto::{ParentOrderRequest, PositionsRequest};
use tonic::transport::Channel;

// Algorithm types
#[derive(Clone, Debug, PartialEq)]
enum AlgoType {
    TWAP,
    POV,
    VWAP,
    IS,
}

impl std::fmt::Display for AlgoType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlgoType::TWAP => write!(f, "TWAP"),
            AlgoType::POV => write!(f, "POV"),
            AlgoType::VWAP => write!(f, "VWAP"),
            AlgoType::IS => write!(f, "IS"),
        }
    }
}

// Market data with price history
#[derive(Clone, Debug)]
struct MarketData {
    symbol: String,
    bid: f64,
    ask: f64,
    last: f64,
    volume: u64,
    price_history: VecDeque<(f64, f64)>, // (time, price)
}

#[derive(Clone, Debug)]
struct LiveIndicators {
    rsi: Option<f64>,
    macd: Option<f64>,
    macd_signal: Option<f64>,
    bb_upper: Option<f64>,
    bb_middle: Option<f64>,
    bb_lower: Option<f64>,
    last_update: u64,
}

// Live transaction
#[derive(Clone, Debug)]
struct Transaction {
    symbol: String,
    side: String,
    price: f64,
    quantity: u64,
    entity: String,
    timestamp: String,
}

// News item
#[derive(Clone, Debug)]
struct NewsItem {
    symbol: String,
    headline: String,
    timestamp: String,
    sentiment: String, // "positive", "negative", "neutral"
}

// Shared state
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
        }
    }
}

struct TraderApp {
    server_address: String,
    symbol: String,
    side: Side,
    quantity: String,
    limit_price: String,
    duration_sec: String,
    num_slices: String,
    algo_type: AlgoType,
    state: AppState,
    show_positions: bool,
    show_orders: bool,
    show_market_data: bool,
    show_live_feed: bool,
    show_performance: bool,
    auto_refresh: bool,
    last_update: f64,
    last_poll_time: f64,
}

impl Default for TraderApp {
    fn default() -> Self {
        let app = Self {
            server_address: "127.0.0.1:9090".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            quantity: "1000".to_string(),
            limit_price: "50000.0".to_string(),
            duration_sec: "10.0".to_string(),
            num_slices: "10".to_string(),
            algo_type: AlgoType::TWAP,
            state: AppState::default(),
            show_positions: true,
            show_orders: true,
            show_market_data: true,
            show_live_feed: true,
            show_performance: true,
            auto_refresh: true,
            last_update: 0.0,
            last_poll_time: 0.0,
        };

        start_realtime_data_fetcher(app.state.clone());

        app
    }
}

impl eframe::App for TraderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ALWAYS request continuous repaint for real-time updates
        ctx.request_repaint();

        // Poll order status every 500ms if connected (using separate poll timer)
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();

        if now - self.last_poll_time > 0.5 {
            self.last_poll_time = now;
            let conn_status = self.state.connection_status.lock().unwrap().clone();

            if matches!(conn_status, ConnectionStatus::Connected) {
                self.poll_order_status();
                self.refresh_data(); // Also refresh positions
            }
        }

        // Top menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Connect").clicked() {
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

                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.show_market_data, "Show Market Data");
                    ui.checkbox(&mut self.show_positions, "Show Positions");
                    ui.checkbox(&mut self.show_orders, "Show Orders");
                    ui.checkbox(&mut self.show_performance, "Show Performance Dashboard");
                    ui.checkbox(&mut self.show_live_feed, "Show Live Feed");
                    ui.checkbox(&mut self.auto_refresh, "Auto Refresh");
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        *self.state.status_message.lock().unwrap() =
                            "Algo Execution Sandbox v0.1.0 - REAL-TIME Market Data".to_string();
                    }
                });
            });
        });

        // Status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let conn_status = self.state.connection_status.lock().unwrap().clone();
                let (color, text) = match conn_status {
                    ConnectionStatus::Disconnected => (egui::Color32::GRAY, "●  Disconnected".to_string()),
                    ConnectionStatus::Connecting => (egui::Color32::YELLOW, "●  Connecting...".to_string()),
                    ConnectionStatus::Connected => (egui::Color32::GREEN, "●  Connected".to_string()),
                    ConnectionStatus::Error(msg) => (egui::Color32::RED, format!("●  Error: {}", msg)),
                };
                ui.colored_label(color, text);
                ui.separator();

                let status = self.state.status_message.lock().unwrap().clone();
                ui.label(status);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(egui::Color32::GREEN, "🔴 LIVE DATA (Binance API)");
                    ui.separator();
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
                    ui.label(format!("Updated: {:.1}s ago", now - self.last_update));
                });
            });
        });

        // Main layout: Left panel (trading) + Right panel (live data)
        egui::SidePanel::right("live_feed_panel")
            .resizable(true)
            .default_width(500.0)
            .min_width(400.0)
            .show(ctx, |ui| {
                self.render_right_panel(ui);
            });

        // Left panel - trading interface
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Algorithmic Execution Trader");
                ui.add_space(10.0);

                if self.show_market_data {
                    egui::CollapsingHeader::new("📊 Market Data")
                        .default_open(true)
                        .show(ui, |ui| {
                            self.render_market_data(ui);
                        });
                    ui.add_space(10.0);

                    egui::CollapsingHeader::new("📈 Live Technical Indicators")
                        .default_open(false)
                        .show(ui, |ui| {
                            self.render_live_indicators(ui);
                        });
                    ui.add_space(10.0);
                }

                egui::CollapsingHeader::new("📝 Order Entry")
                    .default_open(true)
                    .show(ui, |ui| {
                        self.render_order_entry(ui);
                    });
                ui.add_space(10.0);

                if self.show_positions {
                    egui::CollapsingHeader::new("💼 Positions")
                        .default_open(true)
                        .show(ui, |ui| {
                            self.render_positions(ui);
                        });
                    ui.add_space(10.0);
                }

                if self.show_orders {
                    egui::CollapsingHeader::new("📋 Orders")
                        .default_open(true)
                        .show(ui, |ui| {
                            self.render_orders(ui);
                        });
                    ui.add_space(10.0);
                }

                if self.show_performance {
                    egui::CollapsingHeader::new("📈 Performance Dashboard")
                        .default_open(false)
                        .show(ui, |ui| {
                            self.render_performance_dashboard(ui);
                        });
                }
            });
        });

        self.last_update = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
    }
}

impl TraderApp {
    fn render_right_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("📡 Live Market Feed");
        ui.colored_label(egui::Color32::GREEN, "✓ Real-Time Data from Binance");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Popular symbols
            ui.group(|ui| {
                ui.heading("⭐ Popular Symbols (Crypto)");
                let popular = self.state.popular_symbols.lock().unwrap().clone();
                ui.horizontal_wrapped(|ui| {
                    for sym in popular {
                        if ui.button(&sym).clicked() {
                            self.symbol = sym;
                        }
                    }
                });
            });
            ui.add_space(10.0);

            // Live price charts
            ui.group(|ui| {
                ui.heading("📈 Live Price Charts");
                let market_data = self.state.market_data.lock().unwrap().clone();

                for data in market_data.iter().take(3) {
                    ui.label(egui::RichText::new(&data.symbol).strong());
                    ui.label(format!("${:.2} | Vol: {}", data.last, data.volume));

                    let points: PlotPoints = data.price_history.iter()
                        .map(|(t, p)| [*t, *p])
                        .collect();

                    let line = Line::new(points).color(egui::Color32::from_rgb(0, 200, 100));

                    Plot::new(format!("chart_{}", data.symbol))
                        .height(80.0)
                        .show_axes([false, true])
                        .allow_zoom(false)
                        .allow_drag(false)
                        .allow_scroll(false)
                        .show(ui, |plot_ui| {
                            plot_ui.line(line);
                        });

                    ui.separator();
                }
            });
            ui.add_space(10.0);

            // Live transactions from Binance
            ui.group(|ui| {
                ui.heading("💸 Recent Trades (Real Binance Data)");
                let transactions = self.state.transactions.lock().unwrap().clone();

                egui::Grid::new("trans_grid")
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

                        for trans in transactions.iter().take(15) {
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
            ui.add_space(10.0);

            // News feed
            ui.group(|ui| {
                ui.heading(format!("📰 Market Updates"));
                let news = self.state.news.lock().unwrap().clone();

                for item in news.iter().take(10) {
                    let sentiment_color = match item.sentiment.as_str() {
                        "positive" => egui::Color32::GREEN,
                        "negative" => egui::Color32::RED,
                        _ => egui::Color32::GRAY,
                    };

                    ui.horizontal(|ui| {
                        ui.colored_label(sentiment_color, "●");
                        ui.label(egui::RichText::new(&item.headline).text_style(egui::TextStyle::Body));
                    });
                    ui.label(egui::RichText::new(&item.timestamp).small().italics());
                    ui.separator();
                }
            });
        });
    }

    fn render_order_entry(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("order_entry_grid")
            .num_columns(2)
            .spacing([40.0, 10.0])
            .show(ui, |ui| {
                ui.label("Symbol:");
                ui.text_edit_singleline(&mut self.symbol);
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
                    ui.radio_value(&mut self.algo_type, AlgoType::TWAP, "TWAP");
                    ui.radio_value(&mut self.algo_type, AlgoType::POV, "POV");
                    ui.radio_value(&mut self.algo_type, AlgoType::VWAP, "VWAP");
                    ui.radio_value(&mut self.algo_type, AlgoType::IS, "IS");
                });
                ui.end_row();
            });

        ui.add_space(5.0);

        let algo_desc = match self.algo_type {
            AlgoType::TWAP => "Time-Weighted Average Price: Splits order evenly over time",
            AlgoType::POV => "Percent of Volume: Executes based on market volume percentage",
            AlgoType::VWAP => "Volume-Weighted Average Price: Follows historical volume patterns",
            AlgoType::IS => "Implementation Shortfall: Balances urgency vs market impact",
        };
        ui.label(egui::RichText::new(algo_desc).italics().color(egui::Color32::GRAY));

        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let button_text = format!("🚀 Submit {}", self.algo_type);
            if ui.add_sized([150.0, 40.0], egui::Button::new(button_text)).clicked() {
                self.submit_order();
            }

            ui.add_space(10.0);

            if ui.add_sized([120.0, 40.0], egui::Button::new("🔄 Refresh")).clicked() {
                self.refresh_data();
            }
        });
    }

    fn render_positions(&self, ui: &mut egui::Ui) {
        let positions = self.state.positions.lock().unwrap();

        if positions.is_empty() {
            ui.label("No positions");
            return;
        }

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

    fn render_orders(&self, ui: &mut egui::Ui) {
        let orders = self.state.orders.lock().unwrap();

        if orders.is_empty() {
            ui.label("No orders");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
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

                    for order in orders.iter().take(20) {
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
        });
    }

    fn render_market_data(&self, ui: &mut egui::Ui) {
        let market_data = self.state.market_data.lock().unwrap();

        if market_data.is_empty() {
            ui.label("Loading real-time data from Binance...");
            return;
        }

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

    fn render_live_indicators(&self, ui: &mut egui::Ui) {
        let indicators = self.state.live_indicators.lock().unwrap();

        if indicators.is_empty() {
            ui.label("Calculating indicators... (need 50+ price points)");
            return;
        }

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
                        ui.colored_label(macd_color, format!("{:.2}/{:.2}", macd, signal));
                    } else {
                        ui.label("-");
                    }

                    if let (Some(upper), Some(lower)) = (ind.bb_upper, ind.bb_lower) {
                        ui.label(format!("{:.2}/{:.2}", upper, lower));
                    } else {
                        ui.label("-");
                    }

                    let age = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() - ind.last_update;
                    ui.label(format!("{}s ago", age));
                    ui.end_row();
                }
            });
    }

    fn connect_to_engine(&mut self) {
        *self.state.connection_status.lock().unwrap() = ConnectionStatus::Connecting;
        *self.state.status_message.lock().unwrap() =
            format!("Connecting to {}...", self.server_address);

        let server_addr = self.server_address.clone();
        let state = self.state.clone();

        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();
            match runtime.block_on(async {
                ExecutionServiceClient::connect(format!("http://{}", server_addr)).await
            }) {
                Ok(client) => {
                    eprintln!("[GUI] Connected to {}", server_addr);
                    *state.grpc_client.lock().unwrap() = Some(client);
                    *state.connection_status.lock().unwrap() = ConnectionStatus::Connected;
                    *state.status_message.lock().unwrap() = format!("Connected to {}", server_addr);
                }
                Err(e) => {
                    eprintln!("[GUI] Connection failed: {}", e);
                    *state.connection_status.lock().unwrap() = ConnectionStatus::Error(format!("Connection failed: {}", e));
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
        use api::proto::{OrderStatusRequest};

        // Get client
        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            return;
        }
        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        // Get order IDs to poll
        let orders = self.state.orders.lock().unwrap();
        let order_ids: Vec<u64> = orders.iter()
            .filter(|o| {
                o.status == "WORKING" ||
                o.status == "PENDING" ||
                o.status == "ACCEPTED" ||
                o.status == "PARTIALLY_FILLED"
            })
            .map(|o| o.order_id)
            .collect();
        drop(orders);

        if order_ids.is_empty() {
            return;
        }

        let state = self.state.clone();
        thread::spawn(move || {
            // Use the shared runtime instead of creating a new one
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

                        // Log only when status or filled qty changes
                        if old_status != resp.status || old_filled != resp.filled_qty {
                            eprintln!("[GUI] Order {} updated: {} {} -> {} {}",
                                order_id, old_status, old_filled, resp.status, resp.filled_qty);
                        }

                        // If filled, trigger performance update
                        if resp.status == "FILLED" && resp.filled_qty > 0 {
                            drop(orders);
                            update_performance_from_fills(&state);
                        }
                    }
                }
            }
        });
    }

    fn submit_order(&mut self) {
        let qty: u64 = match self.quantity.parse() {
            Ok(v) => v,
            Err(_) => {
                *self.state.status_message.lock().unwrap() = "Invalid quantity".to_string();
                return;
            }
        };

        let limit_price: f64 = match self.limit_price.parse() {
            Ok(v) => v,
            Err(_) => {
                *self.state.status_message.lock().unwrap() = "Invalid price".to_string();
                return;
            }
        };

        let duration: f64 = match self.duration_sec.parse() {
            Ok(v) => v,
            Err(_) => {
                *self.state.status_message.lock().unwrap() = "Invalid duration".to_string();
                return;
            }
        };

        let num_slices: u32 = match self.num_slices.parse() {
            Ok(v) => v,
            Err(_) => {
                *self.state.status_message.lock().unwrap() = "Invalid num slices".to_string();
                return;
            }
        };

        // Check if connected and clone the client
        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            *self.state.status_message.lock().unwrap() = "Not connected to engine".to_string();
            return;
        }

        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        let symbol = self.symbol.clone();
        let side_str = format!("{}", self.side);
        let algo_type = format!("{}", self.algo_type);
        let state = self.state.clone();

        *self.state.status_message.lock().unwrap() = format!(
            "Submitting {}: {} {} {} @ ${:.2}",
            algo_type, side_str, qty, symbol, limit_price
        );

        thread::spawn(move || {

            // Use the shared runtime instead of creating a new one
            let runtime = state.grpc_runtime.lock().unwrap();
            let result = runtime.block_on(async {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
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
                    eprintln!("[GUI] Order {} submitted: {}", resp.parent_order_id, resp.status);
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
                    *state.status_message.lock().unwrap() = format!("Order {} {}", resp.parent_order_id, resp.status);
                }
                Err(e) => {
                    eprintln!("[GUI] Order submission FAILED: {}", e);
                    *state.status_message.lock().unwrap() = format!("Order submission failed: {}", e);
                }
            }
        });
    }

    fn refresh_data(&self) {
        // Check if connected
        let client_opt = self.state.grpc_client.lock().unwrap();
        if client_opt.is_none() {
            return;
        }

        let mut client = client_opt.clone().unwrap();
        drop(client_opt);

        let state = self.state.clone();

        thread::spawn(move || {
            let runtime = state.grpc_runtime.lock().unwrap();

            // Query positions from engine
            let positions_result = runtime.block_on(async {
                let request = PositionsRequest {
                    symbol: String::new(), // Empty = all symbols
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
                            avg_price: 0.0, // TODO: track avg price
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

    fn render_performance_dashboard(&self, ui: &mut egui::Ui) {
        let metrics_opt = self.state.performance_metrics.lock().unwrap();
        let equity_curve = self.state.equity_curve.lock().unwrap();

        if metrics_opt.is_none() {
            ui.label("No performance data available. Run a backtest to generate metrics.");
            return;
        }

        let metrics = metrics_opt.as_ref().unwrap();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.group(|ui| {
                ui.heading("Equity Curve");

                if !equity_curve.is_empty() {
                    let points: PlotPoints = equity_curve.iter()
                        .enumerate()
                        .map(|(i, p)| [i as f64, p.equity])
                        .collect();

                    let line = Line::new(points)
                        .color(egui::Color32::from_rgb(100, 150, 255))
                        .width(2.0);

                    Plot::new("equity_curve_plot")
                        .height(200.0)
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

            ui.add_space(10.0);

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

            ui.add_space(10.0);

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

            ui.add_space(10.0);

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
                        ui.colored_label(egui::Color32::GREEN, format!("{}", metrics.winning_trades));
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
                        ui.colored_label(egui::Color32::GREEN, format!("${:.2}", metrics.average_win));
                        ui.end_row();

                        ui.strong("Average Loss:");
                        ui.colored_label(egui::Color32::RED, format!("${:.2}", metrics.average_loss));
                        ui.end_row();

                        ui.strong("Largest Win:");
                        ui.colored_label(egui::Color32::GREEN, format!("${:.2}", metrics.largest_win));
                        ui.end_row();

                        ui.strong("Largest Loss:");
                        ui.colored_label(egui::Color32::RED, format!("${:.2}", metrics.largest_loss));
                        ui.end_row();
                    });
            });
        });
    }
}

// ============================================================================
// REAL-TIME DATA FETCHER - Uses Binance Public API
// ============================================================================

#[derive(Debug, Deserialize)]
struct BinanceTickerPrice {
    symbol: String,
    price: String,
}

#[derive(Debug, Deserialize)]
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

fn calculate_live_indicators(symbol: &str, price_history: &VecDeque<(f64, f64)>) -> Option<LiveIndicators> {
    if price_history.len() < 50 {
        return None;
    }

    let prices: Vec<f64> = price_history.iter().map(|(_, p)| *p).collect();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let rsi = indicators::rsi(&prices, 14).ok()
        .and_then(|v| v.last().copied());

    let macd_result = indicators::macd(&prices, 12, 26, 9).ok();
    let macd = macd_result.as_ref()
        .and_then(|m| m.macd_line.last().copied());
    let macd_signal = macd_result.as_ref()
        .and_then(|m| m.signal_line.last().copied());

    let bb_result = indicators::bollinger_bands(&prices, 20, 2.0).ok();
    let bb_upper = bb_result.as_ref()
        .and_then(|bb| bb.upper.last().copied());
    let bb_middle = bb_result.as_ref()
        .and_then(|bb| bb.middle.last().copied());
    let bb_lower = bb_result.as_ref()
        .and_then(|bb| bb.lower.last().copied());

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
        // Process ANY filled quantity (not just fully filled)
        if order.filled_qty > 0 {
            let trade_type = if order.side == "BUY" {
                TradeType::Buy
            } else {
                TradeType::Sell
            };

            // Use estimated price of $100 (close to our market maker mid price)
            let avg_price = 100.0;
            let pnl = if order.side == "SELL" {
                // Estimate P&L for sells (assuming bought at 99.0)
                (avg_price - 99.0) * order.filled_qty as f64
            } else {
                // Buys don't have realized P&L yet
                0.0
            };

            let trade = Trade {
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64,
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
            eprintln!("[GUI] Performance metrics updated: {} trades", metrics.total_trades);
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

            // Initialize market data
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

            // Generate initial news
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
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();

                // Fetch ticker data for all symbols
                for symbol in &symbols {
                    // Fetch ticker (price, bid, ask, volume)
                    if let Ok(ticker_result) = client
                        .get(format!("https://api.binance.com/api/v3/ticker/bookTicker?symbol={}", symbol))
                        .send()
                        .await
                    {
                        if let Ok(ticker) = ticker_result.json::<serde_json::Value>().await {
                            let bid = ticker["bidPrice"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                            let ask = ticker["askPrice"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                            let last = (bid + ask) / 2.0;

                            // Fetch 24h ticker for volume
                            if let Ok(ticker_24h_result) = client
                                .get(format!("https://api.binance.com/api/v3/ticker/24hr?symbol={}", symbol))
                                .send()
                                .await
                            {
                                if let Ok(ticker_24h) = ticker_24h_result.json::<serde_json::Value>().await {
                                    let volume = ticker_24h["volume"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0) as u64;

                                    // Update market data
                                    let price_hist_clone;
                                    {
                                        let mut market_data = state.market_data.lock().unwrap();
                                        if let Some(data) = market_data.iter_mut().find(|d| d.symbol == *symbol) {
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

                                    if let Some(indicators) = calculate_live_indicators(symbol, &price_hist_clone) {
                                        let mut live_indicators = state.live_indicators.lock().unwrap();
                                        live_indicators.insert(symbol.to_string(), indicators);
                                    }
                                }
                            }
                        }
                    }

                    // Fetch recent trades
                    if let Ok(trades_result) = client
                        .get(format!("https://api.binance.com/api/v3/trades?symbol={}&limit=5", symbol))
                        .send()
                        .await
                    {
                        if let Ok(trades) = trades_result.json::<Vec<serde_json::Value>>().await {
                            let mut transactions = state.transactions.lock().unwrap();

                            for trade in trades {
                                let price = trade["price"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                let qty = trade["qty"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                let is_buyer_maker = trade["isBuyerMaker"].as_bool().unwrap_or(false);
                                let _time = trade["time"].as_u64().unwrap_or(0);

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

                // Generate news based on market movements
                {
                    let market_data = state.market_data.lock().unwrap();
                    let indicators = state.live_indicators.lock().unwrap();
                    let mut news = state.news.lock().unwrap();

                    for data in market_data.iter() {
                        // Calculate price change over last 10 data points
                        if data.price_history.len() >= 10 {
                            let recent_prices: Vec<f64> = data.price_history.iter()
                                .rev()
                                .take(10)
                                .map(|(_, p)| *p)
                                .collect();

                            let old_price = recent_prices.last().unwrap();
                            let current_price = recent_prices.first().unwrap();
                            let pct_change = ((current_price - old_price) / old_price) * 100.0;

                            // Generate news for significant moves (> 0.5%)
                            if pct_change.abs() > 0.5 && news.len() < 20 {
                                let (headline, sentiment) = if pct_change > 0.0 {
                                    (
                                        format!("{} surges {:.2}% amid strong buying pressure",
                                            data.symbol.replace("USDT", ""), pct_change),
                                        "positive".to_string()
                                    )
                                } else {
                                    (
                                        format!("{} drops {:.2}% on profit-taking",
                                            data.symbol.replace("USDT", ""), pct_change.abs()),
                                        "negative".to_string()
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

                        // Generate news based on RSI (if available)
                        if let Some(ind) = indicators.get(&data.symbol) {
                            if let Some(rsi) = ind.rsi {
                                if news.len() < 20 {
                                    if rsi > 70.0 {
                                        news.push(NewsItem {
                                            symbol: data.symbol.clone(),
                                            headline: format!("{} RSI at {:.1} - overbought territory, potential reversal",
                                                data.symbol.replace("USDT", ""), rsi),
                                            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                            sentiment: "negative".to_string(),
                                        });
                                    } else if rsi < 30.0 {
                                        news.push(NewsItem {
                                            symbol: data.symbol.clone(),
                                            headline: format!("{} RSI at {:.1} - oversold conditions, potential bounce",
                                                data.symbol.replace("USDT", ""), rsi),
                                            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                            sentiment: "positive".to_string(),
                                        });
                                    }
                                }
                            }

                            // Generate news based on MACD crossovers
                            if let (Some(macd), Some(signal)) = (ind.macd, ind.macd_signal) {
                                if news.len() < 20 {
                                    let bullish = macd > signal && (macd - signal).abs() > 0.1;
                                    let bearish = signal > macd && (signal - macd).abs() > 0.1;

                                    if bullish {
                                        news.push(NewsItem {
                                            symbol: data.symbol.clone(),
                                            headline: format!("{} MACD bullish crossover detected - momentum turning positive",
                                                data.symbol.replace("USDT", "")),
                                            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                            sentiment: "positive".to_string(),
                                        });
                                    } else if bearish {
                                        news.push(NewsItem {
                                            symbol: data.symbol.clone(),
                                            headline: format!("{} MACD bearish crossover - momentum weakening",
                                                data.symbol.replace("USDT", "")),
                                            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                            sentiment: "negative".to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Volume analysis
                    for data in market_data.iter() {
                        if news.len() < 20 {
                            // High volume indicates strong interest
                            if data.volume > 1_000_000 {
                                news.push(NewsItem {
                                    symbol: data.symbol.clone(),
                                    headline: format!("{} sees elevated trading volume - institutional interest building",
                                        data.symbol.replace("USDT", "")),
                                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                    sentiment: "neutral".to_string(),
                                });
                            }
                        }
                    }

                    // Market-wide sentiment
                    if news.len() < 20 {
                        let btc_data = market_data.iter().find(|d| d.symbol == "BTCUSDT");
                        if let Some(btc) = btc_data {
                            if btc.price_history.len() >= 10 {
                                let recent: Vec<f64> = btc.price_history.iter()
                                    .rev()
                                    .take(10)
                                    .map(|(_, p)| *p)
                                    .collect();

                                let old = recent.last().unwrap();
                                let current = recent.first().unwrap();
                                let btc_change = ((current - old) / old) * 100.0;

                                if btc_change > 1.0 {
                                    news.push(NewsItem {
                                        symbol: "MARKET".to_string(),
                                        headline: format!("Crypto market rally continues as Bitcoin leads gains at +{:.2}%", btc_change),
                                        timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                        sentiment: "positive".to_string(),
                                    });
                                } else if btc_change < -1.0 {
                                    news.push(NewsItem {
                                        symbol: "MARKET".to_string(),
                                        headline: format!("Market pullback underway as Bitcoin declines {:.2}%", btc_change.abs()),
                                        timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                        sentiment: "negative".to_string(),
                                    });
                                }
                            }
                        }
                    }

                    // Keep only last 15 news items
                    while news.len() > 15 {
                        news.remove(0);
                    }
                }

                // Update status
                *state.status_message.lock().unwrap() = "Live data streaming from Binance".to_string();

                // Wait 2 seconds before next update (Binance rate limits)
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
        "Algo Execution Sandbox - REAL-TIME Market Data",
        options,
        Box::new(|_cc| Ok(Box::new(TraderApp::default()))),
    )
}
