pub mod replay;

use analytics::{PerformanceAnalyzer, Trade, TradeType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

pub use replay::*;

// Constants for default rates
const DEFAULT_RISK_FREE_RATE: f64 = 0.02;      // 2% annual risk-free rate
#[allow(dead_code)]
const DEFAULT_COMMISSION_RATE: f64 = 0.001;   // 0.1% commission per trade
#[allow(dead_code)]
const DEFAULT_SLIPPAGE_RATE: f64 = 0.0001;    // 0.01% slippage per trade

#[derive(Debug, Error)]
pub enum BacktestError {
    #[error("Insufficient data")]
    InsufficientData,

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("Strategy error: {0}")]
    StrategyError(String),
}

pub type Result<T> = std::result::Result<T, BacktestError>;

// ============================================================================
// Market Data Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OHLCV {
    pub timestamp: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataBar {
    pub timestamp: u64,
    pub symbol: String,
    pub ohlcv: OHLCV,
}

// ============================================================================
// Strategy Trait
// ============================================================================

pub trait Strategy: Send {
    /// Called when strategy is initialized
    fn on_start(&mut self, initial_capital: f64);

    /// Called on each bar of market data
    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal>;

    /// Called at end of backtest
    fn on_end(&mut self);

    fn name(&self) -> &str;
}

#[derive(Debug, Clone)]
pub enum Signal {
    Buy {
        symbol: String,
        quantity: f64,
        price: Option<f64>,
    },
    Sell {
        symbol: String,
        quantity: f64,
        price: Option<f64>,
    },
    Close {
        symbol: String,
    },
}

// ============================================================================
// Portfolio Management
// ============================================================================

#[derive(Debug, Clone)]
pub struct Position {
    pub symbol: String,
    pub quantity: f64,
    pub avg_entry_price: f64,
    pub current_price: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
}

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub cash: f64,
    pub initial_capital: f64,
    pub positions: HashMap<String, Position>,
    pub equity: f64,
}

impl Portfolio {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            cash: initial_capital,
            initial_capital,
            positions: HashMap::new(),
            equity: initial_capital,
        }
    }

    pub fn update_prices(&mut self, prices: &HashMap<String, f64>) {
        let mut total_position_value = 0.0;

        for (symbol, price) in prices {
            if let Some(pos) = self.positions.get_mut(symbol) {
                pos.current_price = *price;
                pos.unrealized_pnl = (price - pos.avg_entry_price) * pos.quantity;
                total_position_value += price * pos.quantity.abs();
            }
        }

        self.equity = self.cash + total_position_value;
    }

    pub fn execute_buy(&mut self, symbol: String, quantity: f64, price: f64, timestamp: u64) -> Option<Trade> {
        let cost = quantity * price;

        if cost > self.cash {
            return None; // Insufficient funds
        }

        self.cash -= cost;

        if let Some(pos) = self.positions.get_mut(&symbol) {
            // Average in
            let total_qty = pos.quantity + quantity;
            let total_cost = pos.avg_entry_price * pos.quantity + cost;
            pos.avg_entry_price = total_cost / total_qty;
            pos.quantity = total_qty;
            pos.current_price = price;
        } else {
            // New position
            self.positions.insert(
                symbol.clone(),
                Position {
                    symbol: symbol.clone(),
                    quantity,
                    avg_entry_price: price,
                    current_price: price,
                    unrealized_pnl: 0.0,
                    realized_pnl: 0.0,
                },
            );
        }

        Some(Trade {
            timestamp,
            symbol,
            side: TradeType::Buy,
            quantity,
            price,
            pnl: 0.0, // No PnL on entry
        })
    }

    pub fn execute_sell(&mut self, symbol: String, quantity: f64, price: f64, timestamp: u64) -> Option<Trade> {
        let pos = self.positions.get_mut(&symbol)?;

        if pos.quantity < quantity {
            return None; // Insufficient position
        }

        let pnl = (price - pos.avg_entry_price) * quantity;
        pos.quantity -= quantity;
        pos.realized_pnl += pnl;
        self.cash += quantity * price;

        if pos.quantity == 0.0 {
            self.positions.remove(&symbol);
        }

        Some(Trade {
            timestamp,
            symbol,
            side: TradeType::Sell,
            quantity,
            price,
            pnl,
        })
    }

    pub fn close_position(&mut self, symbol: &str, price: f64, timestamp: u64) -> Option<Trade> {
        let pos = self.positions.get(symbol)?;
        let quantity = pos.quantity;
        self.execute_sell(symbol.to_string(), quantity, price, timestamp)
    }

    pub fn get_position(&self, symbol: &str) -> Option<&Position> {
        self.positions.get(symbol)
    }

    pub fn total_equity(&self) -> f64 {
        self.equity
    }
}

// ============================================================================
// Backtesting Engine
// ============================================================================

pub struct BacktestEngine {
    strategy: Box<dyn Strategy>,
    portfolio: Portfolio,
    analyzer: PerformanceAnalyzer,
    #[allow(dead_code)]
    commission_rate: f64,
    slippage: f64,
}

impl BacktestEngine {
    pub fn new(
        strategy: Box<dyn Strategy>,
        initial_capital: f64,
        commission_rate: f64,
        slippage: f64,
    ) -> Self {
        Self {
            strategy,
            portfolio: Portfolio::new(initial_capital),
            analyzer: PerformanceAnalyzer::new(initial_capital, DEFAULT_RISK_FREE_RATE),
            commission_rate,
            slippage,
        }
    }

    pub fn run(&mut self, market_data: Vec<MarketDataBar>) -> Result<()> {
        if market_data.is_empty() {
            return Err(BacktestError::InsufficientData);
        }

        self.strategy.on_start(self.portfolio.initial_capital);

        for bar in market_data {
            // Update portfolio prices
            let mut prices = HashMap::new();
            prices.insert(bar.symbol.clone(), bar.ohlcv.close);
            self.portfolio.update_prices(&prices);

            // Get signals from strategy
            let signals = self.strategy.on_bar(&bar, &self.portfolio);

            // Execute signals
            for signal in signals {
                match signal {
                    Signal::Buy {
                        symbol,
                        quantity,
                        price,
                    } => {
                        let exec_price = price.unwrap_or(bar.ohlcv.close) * (1.0 + self.slippage);
                        if let Some(trade) = self.portfolio.execute_buy(symbol, quantity, exec_price, bar.timestamp) {
                            self.analyzer.add_trade(trade);
                        }
                    }
                    Signal::Sell {
                        symbol,
                        quantity,
                        price,
                    } => {
                        let exec_price = price.unwrap_or(bar.ohlcv.close) * (1.0 - self.slippage);
                        if let Some(trade) = self.portfolio.execute_sell(symbol, quantity, exec_price, bar.timestamp) {
                            self.analyzer.add_trade(trade);
                        }
                    }
                    Signal::Close { symbol } => {
                        let exec_price = bar.ohlcv.close * (1.0 - self.slippage);
                        if let Some(trade) = self.portfolio.close_position(&symbol, exec_price, bar.timestamp) {
                            self.analyzer.add_trade(trade);
                        }
                    }
                }
            }
        }

        self.strategy.on_end();

        Ok(())
    }

    pub fn get_results(&self) -> analytics::Result<analytics::PerformanceMetrics> {
        self.analyzer.calculate_metrics()
    }

    pub fn get_equity_curve(&self) -> &[analytics::EquityPoint] {
        self.analyzer.get_equity_curve()
    }

    pub fn get_trades(&self) -> &[Trade] {
        self.analyzer.get_trades()
    }

    pub fn get_portfolio(&self) -> &Portfolio {
        &self.portfolio
    }
}

// ============================================================================
// Example Strategy: Simple Moving Average Crossover
// ============================================================================

pub struct SMAStrategy {
    fast_period: usize,
    slow_period: usize,
    symbol: String,
    price_history: Vec<f64>,
    in_position: bool,
}

impl SMAStrategy {
    pub fn new(symbol: String, fast_period: usize, slow_period: usize) -> Self {
        Self {
            fast_period,
            slow_period,
            symbol,
            price_history: Vec::new(),
            in_position: false,
        }
    }
}

impl Strategy for SMAStrategy {
    fn on_start(&mut self, _initial_capital: f64) {
        self.price_history.clear();
        self.in_position = false;
    }

    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal> {
        if bar.symbol != self.symbol {
            return vec![];
        }

        self.price_history.push(bar.ohlcv.close);

        if self.price_history.len() < self.slow_period {
            return vec![];
        }

        // Calculate SMAs
        let fast_sma = self.price_history[self.price_history.len() - self.fast_period..]
            .iter()
            .sum::<f64>()
            / self.fast_period as f64;

        let slow_sma = self.price_history[self.price_history.len() - self.slow_period..]
            .iter()
            .sum::<f64>()
            / self.slow_period as f64;

        let mut signals = Vec::new();

        // Generate signals
        if fast_sma > slow_sma && !self.in_position {
            // Buy signal
            let quantity = (portfolio.cash * 0.95) / bar.ohlcv.close; // Use 95% of cash
            signals.push(Signal::Buy {
                symbol: self.symbol.clone(),
                quantity,
                price: None,
            });
            self.in_position = true;
        } else if fast_sma < slow_sma && self.in_position {
            // Sell signal
            signals.push(Signal::Close {
                symbol: self.symbol.clone(),
            });
            self.in_position = false;
        }

        signals
    }

    fn on_end(&mut self) {
        // Cleanup if needed
    }

    fn name(&self) -> &str {
        "SMA Crossover"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_test_data(n: usize) -> Vec<MarketDataBar> {
        (0..n)
            .map(|i| MarketDataBar {
                timestamp: i as u64 * 1_000_000_000,
                symbol: "BTC".to_string(),
                ohlcv: OHLCV {
                    timestamp: i as u64 * 1_000_000_000,
                    open: 50000.0 + i as f64 * 10.0,
                    high: 50100.0 + i as f64 * 10.0,
                    low: 49900.0 + i as f64 * 10.0,
                    close: 50000.0 + i as f64 * 10.0,
                    volume: 100.0,
                },
            })
            .collect()
    }

    #[test]
    fn test_backtest_engine() {
        let strategy = Box::new(SMAStrategy::new("BTC".to_string(), 10, 20));
        let mut engine = BacktestEngine::new(strategy, 100_000.0, DEFAULT_COMMISSION_RATE, DEFAULT_SLIPPAGE_RATE);

        let data = generate_test_data(100);
        engine.run(data).unwrap();

        let metrics = engine.get_results().unwrap();
        assert!(metrics.total_trades > 0);
    }

    #[test]
    fn test_portfolio() {
        let mut portfolio = Portfolio::new(100_000.0);

        let trade = portfolio.execute_buy("BTC".to_string(), 1.0, 50000.0, 1000).unwrap();
        assert_eq!(trade.quantity, 1.0);
        assert_eq!(portfolio.cash, 50_000.0);

        let trade = portfolio.execute_sell("BTC".to_string(), 1.0, 55000.0, 2000).unwrap();
        assert_eq!(trade.pnl, 5000.0);
        assert_eq!(portfolio.cash, 105_000.0);
    }
}
