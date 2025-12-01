use backtesting::{MarketDataBar, Portfolio, Signal, Strategy};
use indicators;
use std::collections::{HashMap, VecDeque};

// ============================================================================
// Pairs Trading Strategy
// ============================================================================

pub struct PairsTradingStrategy {
    symbol_a: String,
    symbol_b: String,
    lookback_period: usize,
    entry_threshold: f64,
    exit_threshold: f64,
    price_history_a: VecDeque<f64>,
    price_history_b: VecDeque<f64>,
    spread_history: VecDeque<f64>,
    in_position: Option<PairsPosition>,
}

#[derive(Debug, Clone, Copy)]
enum PairsPosition {
    LongAShortB,
    ShortALongB,
}

impl PairsTradingStrategy {
    pub fn new(
        symbol_a: String,
        symbol_b: String,
        lookback_period: usize,
        entry_threshold: f64,
        exit_threshold: f64,
    ) -> Self {
        Self {
            symbol_a,
            symbol_b,
            lookback_period,
            entry_threshold,
            exit_threshold,
            price_history_a: VecDeque::new(),
            price_history_b: VecDeque::new(),
            spread_history: VecDeque::new(),
            in_position: None,
        }
    }

    fn calculate_z_score(&self) -> Option<f64> {
        if self.spread_history.len() < self.lookback_period {
            return None;
        }

        let recent_spread: Vec<f64> = self.spread_history.iter().copied().collect();
        let mean = recent_spread.iter().sum::<f64>() / recent_spread.len() as f64;
        let variance = recent_spread
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>()
            / recent_spread.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            return None;
        }

        let current_spread = *self.spread_history.back().unwrap();
        Some((current_spread - mean) / std_dev)
    }
}

impl Strategy for PairsTradingStrategy {
    fn on_start(&mut self, _initial_capital: f64) {
        self.price_history_a.clear();
        self.price_history_b.clear();
        self.spread_history.clear();
        self.in_position = None;
    }

    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal> {
        // We need both symbols' data - simplified version
        // In real implementation, you'd need to sync data from both symbols

        let mut signals = Vec::new();

        if bar.symbol == self.symbol_a {
            self.price_history_a.push_back(bar.ohlcv.close);
            if self.price_history_a.len() > self.lookback_period {
                self.price_history_a.pop_front();
            }
        } else if bar.symbol == self.symbol_b {
            self.price_history_b.push_back(bar.ohlcv.close);
            if self.price_history_b.len() > self.lookback_period {
                self.price_history_b.pop_front();
            }
        }

        // Calculate spread (simplified - should use hedge ratio)
        if !self.price_history_a.is_empty() && !self.price_history_b.is_empty() {
            let price_a = *self.price_history_a.back().unwrap();
            let price_b = *self.price_history_b.back().unwrap();
            let spread = price_a - price_b;

            self.spread_history.push_back(spread);
            if self.spread_history.len() > self.lookback_period {
                self.spread_history.pop_front();
            }

            if let Some(z_score) = self.calculate_z_score() {
                match self.in_position {
                    None => {
                        // Entry logic
                        if z_score > self.entry_threshold {
                            // Spread too high: Short A, Long B
                            let position_size = portfolio.cash * 0.25 / price_a;
                            signals.push(Signal::Sell {
                                symbol: self.symbol_a.clone(),
                                quantity: position_size,
                                price: None,
                            });
                            signals.push(Signal::Buy {
                                symbol: self.symbol_b.clone(),
                                quantity: position_size,
                                price: None,
                            });
                            self.in_position = Some(PairsPosition::ShortALongB);
                        } else if z_score < -self.entry_threshold {
                            // Spread too low: Long A, Short B
                            let position_size = portfolio.cash * 0.25 / price_a;
                            signals.push(Signal::Buy {
                                symbol: self.symbol_a.clone(),
                                quantity: position_size,
                                price: None,
                            });
                            signals.push(Signal::Sell {
                                symbol: self.symbol_b.clone(),
                                quantity: position_size,
                                price: None,
                            });
                            self.in_position = Some(PairsPosition::LongAShortB);
                        }
                    }
                    Some(_) => {
                        // Exit logic
                        if z_score.abs() < self.exit_threshold {
                            signals.push(Signal::Close {
                                symbol: self.symbol_a.clone(),
                            });
                            signals.push(Signal::Close {
                                symbol: self.symbol_b.clone(),
                            });
                            self.in_position = None;
                        }
                    }
                }
            }
        }

        signals
    }

    fn on_end(&mut self) {}

    fn name(&self) -> &str {
        "Pairs Trading"
    }
}

// ============================================================================
// Mean Reversion Strategy (RSI-based)
// ============================================================================

pub struct MeanReversionStrategy {
    symbol: String,
    rsi_period: usize,
    oversold_threshold: f64,
    overbought_threshold: f64,
    price_history: Vec<f64>,
    in_position: bool,
}

impl MeanReversionStrategy {
    pub fn new(
        symbol: String,
        rsi_period: usize,
        oversold_threshold: f64,
        overbought_threshold: f64,
    ) -> Self {
        Self {
            symbol,
            rsi_period,
            oversold_threshold,
            overbought_threshold,
            price_history: Vec::new(),
            in_position: false,
        }
    }
}

impl Strategy for MeanReversionStrategy {
    fn on_start(&mut self, _initial_capital: f64) {
        self.price_history.clear();
        self.in_position = false;
    }

    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal> {
        if bar.symbol != self.symbol {
            return vec![];
        }

        self.price_history.push(bar.ohlcv.close);

        if self.price_history.len() < self.rsi_period + 1 {
            return vec![];
        }

        let mut signals = Vec::new();

        // Calculate RSI
        if let Ok(rsi_values) = indicators::rsi(&self.price_history, self.rsi_period) {
            if let Some(&current_rsi) = rsi_values.last() {
                if !self.in_position && current_rsi < self.oversold_threshold {
                    // Oversold - Buy signal
                    let quantity = (portfolio.cash * 0.95) / bar.ohlcv.close;
                    signals.push(Signal::Buy {
                        symbol: self.symbol.clone(),
                        quantity,
                        price: None,
                    });
                    self.in_position = true;
                } else if self.in_position && current_rsi > self.overbought_threshold {
                    // Overbought - Sell signal
                    signals.push(Signal::Close {
                        symbol: self.symbol.clone(),
                    });
                    self.in_position = false;
                }
            }
        }

        signals
    }

    fn on_end(&mut self) {}

    fn name(&self) -> &str {
        "RSI Mean Reversion"
    }
}

// ============================================================================
// Bollinger Bands Strategy
// ============================================================================

pub struct BollingerBandsStrategy {
    symbol: String,
    period: usize,
    std_dev: f64,
    price_history: Vec<f64>,
    in_position: bool,
}

impl BollingerBandsStrategy {
    pub fn new(symbol: String, period: usize, std_dev: f64) -> Self {
        Self {
            symbol,
            period,
            std_dev,
            price_history: Vec::new(),
            in_position: false,
        }
    }
}

impl Strategy for BollingerBandsStrategy {
    fn on_start(&mut self, _initial_capital: f64) {
        self.price_history.clear();
        self.in_position = false;
    }

    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal> {
        if bar.symbol != self.symbol {
            return vec![];
        }

        self.price_history.push(bar.ohlcv.close);

        if self.price_history.len() < self.period {
            return vec![];
        }

        let mut signals = Vec::new();

        // Calculate Bollinger Bands
        if let Ok(bb) = indicators::bollinger_bands(&self.price_history, self.period, self.std_dev) {
            let current_price = bar.ohlcv.close;
            let lower_band = *bb.lower.last().unwrap();
            let upper_band = *bb.upper.last().unwrap();

            if !self.in_position && current_price < lower_band {
                // Price below lower band - Buy signal
                let quantity = (portfolio.cash * 0.95) / current_price;
                signals.push(Signal::Buy {
                    symbol: self.symbol.clone(),
                    quantity,
                    price: None,
                });
                self.in_position = true;
            } else if self.in_position && current_price > upper_band {
                // Price above upper band - Sell signal
                signals.push(Signal::Close {
                    symbol: self.symbol.clone(),
                });
                self.in_position = false;
            }
        }

        signals
    }

    fn on_end(&mut self) {}

    fn name(&self) -> &str {
        "Bollinger Bands"
    }
}

// ============================================================================
// Momentum Strategy (MACD)
// ============================================================================

pub struct MACDStrategy {
    symbol: String,
    fast_period: usize,
    slow_period: usize,
    signal_period: usize,
    price_history: Vec<f64>,
    in_position: bool,
}

impl MACDStrategy {
    pub fn new(
        symbol: String,
        fast_period: usize,
        slow_period: usize,
        signal_period: usize,
    ) -> Self {
        Self {
            symbol,
            fast_period,
            slow_period,
            signal_period,
            price_history: Vec::new(),
            in_position: false,
        }
    }
}

impl Strategy for MACDStrategy {
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

        let mut signals = Vec::new();

        // Calculate MACD
        if let Ok(macd) = indicators::macd(
            &self.price_history,
            self.fast_period,
            self.slow_period,
            self.signal_period,
        ) {
            if macd.histogram.len() < 2 {
                return vec![];
            }

            let current_histogram = *macd.histogram.last().unwrap();
            let previous_histogram = macd.histogram[macd.histogram.len() - 2];

            // Bullish crossover
            if !self.in_position && previous_histogram < 0.0 && current_histogram > 0.0 {
                let quantity = (portfolio.cash * 0.95) / bar.ohlcv.close;
                signals.push(Signal::Buy {
                    symbol: self.symbol.clone(),
                    quantity,
                    price: None,
                });
                self.in_position = true;
            }
            // Bearish crossover
            else if self.in_position && previous_histogram > 0.0 && current_histogram < 0.0 {
                signals.push(Signal::Close {
                    symbol: self.symbol.clone(),
                });
                self.in_position = false;
            }
        }

        signals
    }

    fn on_end(&mut self) {}

    fn name(&self) -> &str {
        "MACD Momentum"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtesting::OHLCV;

    fn create_test_bar(symbol: &str, close: f64, timestamp: u64) -> MarketDataBar {
        MarketDataBar {
            timestamp,
            symbol: symbol.to_string(),
            ohlcv: OHLCV {
                timestamp,
                open: close,
                high: close,
                low: close,
                close,
                volume: 100.0,
            },
        }
    }

    #[test]
    fn test_mean_reversion_strategy() {
        let mut strategy = MeanReversionStrategy::new("BTC".to_string(), 14, 30.0, 70.0);
        strategy.on_start(100_000.0);

        let portfolio = Portfolio::new(100_000.0);

        // Simulate some data
        for i in 0..50 {
            let bar = create_test_bar("BTC", 50000.0 + i as f64 * 100.0, i);
            let _signals = strategy.on_bar(&bar, &portfolio);
        }

        strategy.on_end();
    }
}
