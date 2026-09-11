use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AnalyticsError {
    #[error("Insufficient data: need at least {required} points, got {actual}")]
    InsufficientData { required: usize, actual: usize },

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("Calculation error: {0}")]
    CalculationError(String),
}

pub type Result<T> = std::result::Result<T, AnalyticsError>;

// ============================================================================
// Trade Data Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub timestamp: u64,
    pub symbol: String,
    pub side: TradeType,
    pub quantity: f64,
    pub price: f64,
    pub pnl: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeType {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub timestamp: u64,
    pub equity: f64,
}

// ============================================================================
// Performance Metrics
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    // Returns
    pub total_return: f64,
    pub annualized_return: f64,
    pub cumulative_return: f64,

    // Risk Metrics
    pub sharpe_ratio: f64,
    pub sortino_ratio: f64,
    pub calmar_ratio: f64,
    pub max_drawdown: f64,
    pub max_drawdown_duration_days: f64,

    // Trade Statistics
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub average_win: f64,
    pub average_loss: f64,
    pub largest_win: f64,
    pub largest_loss: f64,
    pub average_trade_pnl: f64,

    // Risk Metrics
    pub volatility: f64,
    pub downside_volatility: f64,
    pub var_95: f64,
    pub var_99: f64,
    pub cvar_95: f64,
    pub cvar_99: f64,
}

// ============================================================================
// Performance Analytics
// ============================================================================

pub struct PerformanceAnalyzer {
    trades: Vec<Trade>,
    equity_curve: Vec<EquityPoint>,
    initial_capital: f64,
    risk_free_rate: f64,
}

impl PerformanceAnalyzer {
    pub fn new(initial_capital: f64, risk_free_rate: f64) -> Self {
        Self {
            trades: Vec::new(),
            equity_curve: vec![EquityPoint {
                timestamp: 0,
                equity: initial_capital,
            }],
            initial_capital,
            risk_free_rate,
        }
    }

    pub fn add_trade(&mut self, trade: Trade) {
        let last_equity = self.equity_curve.last().unwrap().equity;
        let new_equity = last_equity + trade.pnl;

        self.equity_curve.push(EquityPoint {
            timestamp: trade.timestamp,
            equity: new_equity,
        });

        self.trades.push(trade);
    }

    pub fn calculate_metrics(&self) -> Result<PerformanceMetrics> {
        if self.trades.is_empty() {
            return Err(AnalyticsError::InsufficientData {
                required: 1,
                actual: 0,
            });
        }

        let returns = self.calculate_returns();
        let pnl_series: Vec<f64> = self.trades.iter().map(|t| t.pnl).collect();

        // Return metrics
        let total_return = self.calculate_total_return();
        let annualized_return = self.calculate_annualized_return()?;
        let cumulative_return = total_return;

        // Risk-adjusted metrics
        let sharpe_ratio = self.calculate_sharpe_ratio(&returns)?;
        let sortino_ratio = self.calculate_sortino_ratio(&returns)?;
        let (max_drawdown, max_dd_duration) = self.calculate_max_drawdown();
        let calmar_ratio = if max_drawdown != 0.0 {
            annualized_return / max_drawdown.abs()
        } else {
            0.0
        };

        // Trade statistics
        let winning_trades: Vec<f64> = pnl_series.iter().copied().filter(|&x| x > 0.0).collect();
        let losing_trades: Vec<f64> = pnl_series.iter().copied().filter(|&x| x < 0.0).collect();

        let total_trades = self.trades.len();
        let win_count = winning_trades.len();
        let loss_count = losing_trades.len();
        let win_rate = win_count as f64 / total_trades as f64;

        let total_wins: f64 = winning_trades.iter().sum();
        let total_losses: f64 = losing_trades.iter().map(|x| x.abs()).sum();
        let profit_factor = if total_losses != 0.0 {
            total_wins / total_losses
        } else {
            f64::INFINITY
        };

        let average_win = if !winning_trades.is_empty() {
            winning_trades.iter().sum::<f64>() / winning_trades.len() as f64
        } else {
            0.0
        };

        let average_loss = if !losing_trades.is_empty() {
            losing_trades.iter().sum::<f64>() / losing_trades.len() as f64
        } else {
            0.0
        };

        let largest_win = winning_trades.iter().copied().fold(0.0, f64::max);
        let largest_loss = losing_trades.iter().copied().fold(0.0, f64::min);
        let average_trade_pnl = pnl_series.iter().sum::<f64>() / pnl_series.len() as f64;

        // Volatility metrics
        let volatility = self.calculate_volatility(&returns)?;
        let downside_volatility = self.calculate_downside_volatility(&returns)?;

        // VaR and CVaR
        let var_95 = self.calculate_var(&pnl_series, 0.95);
        let var_99 = self.calculate_var(&pnl_series, 0.99);
        let cvar_95 = self.calculate_cvar(&pnl_series, 0.95);
        let cvar_99 = self.calculate_cvar(&pnl_series, 0.99);

        Ok(PerformanceMetrics {
            total_return,
            annualized_return,
            cumulative_return,
            sharpe_ratio,
            sortino_ratio,
            calmar_ratio,
            max_drawdown,
            max_drawdown_duration_days: max_dd_duration,
            total_trades,
            winning_trades: win_count,
            losing_trades: loss_count,
            win_rate,
            profit_factor,
            average_win,
            average_loss,
            largest_win,
            largest_loss,
            average_trade_pnl,
            volatility,
            downside_volatility,
            var_95,
            var_99,
            cvar_95,
            cvar_99,
        })
    }

    fn calculate_returns(&self) -> Vec<f64> {
        self.equity_curve
            .windows(2)
            .map(|w| (w[1].equity - w[0].equity) / w[0].equity)
            .collect()
    }

    fn calculate_total_return(&self) -> f64 {
        let final_equity = self.equity_curve.last().unwrap().equity;
        (final_equity - self.initial_capital) / self.initial_capital
    }

    fn calculate_annualized_return(&self) -> Result<f64> {
        if self.equity_curve.len() < 2 {
            return Err(AnalyticsError::InsufficientData {
                required: 2,
                actual: self.equity_curve.len(),
            });
        }

        let total_return = self.calculate_total_return();
        let first_ts = self.equity_curve.first().unwrap().timestamp;
        let last_ts = self.equity_curve.last().unwrap().timestamp;
        let days = (last_ts - first_ts) as f64 / (1_000_000_000.0 * 86400.0); // nanoseconds to days

        if days == 0.0 {
            return Ok(0.0);
        }

        let years = days / 365.25;
        Ok(((1.0 + total_return).powf(1.0 / years) - 1.0) * 100.0)
    }

    fn calculate_sharpe_ratio(&self, returns: &[f64]) -> Result<f64> {
        if returns.is_empty() {
            return Ok(0.0);
        }

        let mean_return = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance = returns
            .iter()
            .map(|r| (r - mean_return).powi(2))
            .sum::<f64>()
            / returns.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            return Ok(0.0);
        }

        let excess_return = mean_return - (self.risk_free_rate / 252.0); // Daily risk-free rate
        let sharpe = excess_return / std_dev * (252.0_f64).sqrt(); // Annualized

        Ok(sharpe)
    }

    fn calculate_sortino_ratio(&self, returns: &[f64]) -> Result<f64> {
        if returns.is_empty() {
            return Ok(0.0);
        }

        let mean_return = returns.iter().sum::<f64>() / returns.len() as f64;
        let downside_variance: f64 = returns
            .iter()
            .filter(|&&r| r < 0.0)
            .map(|r| r.powi(2))
            .sum::<f64>()
            / returns.len() as f64;
        let downside_std = downside_variance.sqrt();

        if downside_std == 0.0 {
            return Ok(0.0);
        }

        let excess_return = mean_return - (self.risk_free_rate / 252.0);
        let sortino = excess_return / downside_std * (252.0_f64).sqrt();

        Ok(sortino)
    }

    fn calculate_max_drawdown(&self) -> (f64, f64) {
        let mut max_dd = 0.0;
        let mut max_dd_duration = 0.0;
        let mut peak = self.equity_curve[0].equity;
        let mut peak_time = self.equity_curve[0].timestamp;

        for point in &self.equity_curve {
            if point.equity > peak {
                peak = point.equity;
                peak_time = point.timestamp;
            }

            let dd = (point.equity - peak) / peak;
            if dd < max_dd {
                max_dd = dd;
                let duration_ns = point.timestamp - peak_time;
                let duration_days = duration_ns as f64 / (1_000_000_000.0 * 86400.0);
                max_dd_duration = duration_days;
            }
        }

        (max_dd * 100.0, max_dd_duration)
    }

    fn calculate_volatility(&self, returns: &[f64]) -> Result<f64> {
        if returns.is_empty() {
            return Ok(0.0);
        }

        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance =
            returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
        let daily_vol = variance.sqrt();
        let annual_vol = daily_vol * (252.0_f64).sqrt() * 100.0; // Annualized percentage

        Ok(annual_vol)
    }

    fn calculate_downside_volatility(&self, returns: &[f64]) -> Result<f64> {
        let downside_returns: Vec<f64> = returns.iter().copied().filter(|&r| r < 0.0).collect();

        if downside_returns.is_empty() {
            return Ok(0.0);
        }

        let mean = downside_returns.iter().sum::<f64>() / downside_returns.len() as f64;
        let variance = downside_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / downside_returns.len() as f64;
        let daily_vol = variance.sqrt();
        let annual_vol = daily_vol * (252.0_f64).sqrt() * 100.0;

        Ok(annual_vol)
    }

    fn calculate_var(&self, pnl: &[f64], confidence: f64) -> f64 {
        if pnl.is_empty() {
            return 0.0;
        }

        let mut sorted = pnl.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let index = ((1.0 - confidence) * sorted.len() as f64).floor() as usize;
        sorted[index.min(sorted.len() - 1)]
    }

    fn calculate_cvar(&self, pnl: &[f64], confidence: f64) -> f64 {
        let var = self.calculate_var(pnl, confidence);

        let tail: Vec<f64> = pnl.iter().copied().filter(|&x| x <= var).collect();

        if tail.is_empty() {
            return 0.0;
        }

        tail.iter().sum::<f64>() / tail.len() as f64
    }

    pub fn get_equity_curve(&self) -> &[EquityPoint] {
        &self.equity_curve
    }

    pub fn get_trades(&self) -> &[Trade] {
        &self.trades
    }

    pub fn get_monthly_returns(&self) -> HashMap<String, f64> {
        let mut monthly_pnl: HashMap<String, f64> = HashMap::new();

        for trade in &self.trades {
            let datetime =
                chrono::DateTime::from_timestamp((trade.timestamp / 1_000_000_000) as i64, 0)
                    .unwrap();
            let month_key = datetime.format("%Y-%m").to_string();

            *monthly_pnl.entry(month_key).or_insert(0.0) += trade.pnl;
        }

        monthly_pnl
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Calculate correlation between two price series
pub fn correlation(x: &[f64], y: &[f64]) -> Result<f64> {
    if x.len() != y.len() || x.is_empty() {
        return Err(AnalyticsError::InvalidParameter(
            "Series must have equal non-zero length".to_string(),
        ));
    }

    let n = x.len() as f64;
    let mean_x = x.iter().sum::<f64>() / n;
    let mean_y = y.iter().sum::<f64>() / n;

    let cov: f64 = x
        .iter()
        .zip(y.iter())
        .map(|(&xi, &yi)| (xi - mean_x) * (yi - mean_y))
        .sum::<f64>()
        / n;

    let var_x: f64 = x.iter().map(|&xi| (xi - mean_x).powi(2)).sum::<f64>() / n;
    let var_y: f64 = y.iter().map(|&yi| (yi - mean_y).powi(2)).sum::<f64>() / n;

    let std_x = var_x.sqrt();
    let std_y = var_y.sqrt();

    if std_x == 0.0 || std_y == 0.0 {
        return Ok(0.0);
    }

    Ok(cov / (std_x * std_y))
}

#[derive(Debug, Clone)]
pub struct StressScenario {
    pub name: String,
    pub returns: Vec<f64>,
    pub description: String,
}

impl StressScenario {
    pub fn crisis_2008() -> Self {
        Self {
            name: "2008 Financial Crisis".to_string(),
            returns: vec![-0.05, -0.08, -0.12, -0.15, -0.10, -0.08, -0.05],
            description: "2008 market crash scenario".to_string(),
        }
    }

    pub fn covid_crash() -> Self {
        Self {
            name: "COVID-19 Crash".to_string(),
            returns: vec![-0.12, -0.15, -0.20, -0.10, 0.05, 0.08, 0.10],
            description: "2020 COVID market crash".to_string(),
        }
    }

    pub fn flash_crash() -> Self {
        Self {
            name: "Flash Crash".to_string(),
            returns: vec![-0.25, 0.10, 0.05],
            description: "Sudden market crash and recovery".to_string(),
        }
    }

    pub fn custom(name: String, returns: Vec<f64>, description: String) -> Self {
        Self {
            name,
            returns,
            description,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StressTestResult {
    pub scenario_name: String,
    pub initial_value: f64,
    pub final_value: f64,
    pub total_loss: f64,
    pub loss_percentage: f64,
    pub max_drawdown: f64,
}

pub fn stress_test(initial_portfolio_value: f64, scenario: &StressScenario) -> StressTestResult {
    let mut portfolio_value = initial_portfolio_value;
    let mut min_value = initial_portfolio_value;
    let mut max_drawdown = 0.0;

    for &ret in &scenario.returns {
        portfolio_value *= 1.0 + ret;
        if portfolio_value < min_value {
            min_value = portfolio_value;
            let dd = (initial_portfolio_value - portfolio_value) / initial_portfolio_value;
            if dd > max_drawdown {
                max_drawdown = dd;
            }
        }
    }

    let total_loss = initial_portfolio_value - portfolio_value;
    let loss_percentage = total_loss / initial_portfolio_value * 100.0;

    StressTestResult {
        scenario_name: scenario.name.clone(),
        initial_value: initial_portfolio_value,
        final_value: portfolio_value,
        total_loss,
        loss_percentage,
        max_drawdown: max_drawdown * 100.0,
    }
}

pub fn export_trades_to_csv(trades: &[Trade], filepath: &str) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filepath)?;

    writeln!(file, "Timestamp,Symbol,Side,Quantity,Price,PnL")?;

    for trade in trades {
        writeln!(
            file,
            "{},{},{:?},{},{},{}",
            trade.timestamp, trade.symbol, trade.side, trade.quantity, trade.price, trade.pnl
        )?;
    }

    Ok(())
}

pub fn export_metrics_to_csv(metrics: &PerformanceMetrics, filepath: &str) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filepath)?;

    writeln!(file, "Metric,Value")?;
    writeln!(file, "Total Return,{:.2}%", metrics.total_return * 100.0)?;
    writeln!(file, "Annualized Return,{:.2}%", metrics.annualized_return)?;
    writeln!(file, "Sharpe Ratio,{:.2}", metrics.sharpe_ratio)?;
    writeln!(file, "Sortino Ratio,{:.2}", metrics.sortino_ratio)?;
    writeln!(file, "Calmar Ratio,{:.2}", metrics.calmar_ratio)?;
    writeln!(file, "Max Drawdown,{:.2}%", metrics.max_drawdown)?;
    writeln!(
        file,
        "Max DD Duration,{:.1} days",
        metrics.max_drawdown_duration_days
    )?;
    writeln!(file, "Total Trades,{}", metrics.total_trades)?;
    writeln!(file, "Win Rate,{:.1}%", metrics.win_rate * 100.0)?;
    writeln!(file, "Profit Factor,{:.2}", metrics.profit_factor)?;
    writeln!(file, "Average Win,${:.2}", metrics.average_win)?;
    writeln!(file, "Average Loss,${:.2}", metrics.average_loss)?;
    writeln!(file, "Volatility,{:.2}%", metrics.volatility)?;
    writeln!(file, "VaR 95%,${:.2}", metrics.var_95)?;
    writeln!(file, "CVaR 95%,${:.2}", metrics.cvar_95)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_performance_analyzer() {
        let mut analyzer = PerformanceAnalyzer::new(100_000.0, 0.02);

        analyzer.add_trade(Trade {
            timestamp: 1000000000,
            symbol: "BTC".to_string(),
            side: TradeType::Buy,
            quantity: 1.0,
            price: 50000.0,
            pnl: 1000.0,
        });

        analyzer.add_trade(Trade {
            timestamp: 2000000000,
            symbol: "BTC".to_string(),
            side: TradeType::Sell,
            quantity: 1.0,
            price: 51000.0,
            pnl: -500.0,
        });

        let metrics = analyzer.calculate_metrics().unwrap();
        assert_eq!(metrics.total_trades, 2);
        assert_eq!(metrics.winning_trades, 1);
        assert_eq!(metrics.losing_trades, 1);
        assert!((metrics.win_rate - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_correlation() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        let corr = correlation(&x, &y).unwrap();
        assert!((corr - 1.0).abs() < 1e-10); // Perfect correlation
    }
}
