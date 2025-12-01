use thiserror::Error;

#[derive(Debug, Error)]
pub enum IndicatorError {
    #[error("Insufficient data: need at least {required} points, got {actual}")]
    InsufficientData { required: usize, actual: usize },

    #[error("Invalid period: {0}")]
    InvalidPeriod(String),

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),
}

pub type Result<T> = std::result::Result<T, IndicatorError>;

// ============================================================================
// Moving Averages
// ============================================================================

/// Simple Moving Average
pub fn sma(prices: &[f64], period: usize) -> Result<Vec<f64>> {
    if prices.len() < period {
        return Err(IndicatorError::InsufficientData {
            required: period,
            actual: prices.len(),
        });
    }

    if period == 0 {
        return Err(IndicatorError::InvalidPeriod("Period must be > 0".to_string()));
    }

    let mut result = Vec::with_capacity(prices.len() - period + 1);

    for i in period - 1..prices.len() {
        let sum: f64 = prices[i - period + 1..=i].iter().sum();
        result.push(sum / period as f64);
    }

    Ok(result)
}

/// Exponential Moving Average
pub fn ema(prices: &[f64], period: usize) -> Result<Vec<f64>> {
    if prices.len() < period {
        return Err(IndicatorError::InsufficientData {
            required: period,
            actual: prices.len(),
        });
    }

    if period == 0 {
        return Err(IndicatorError::InvalidPeriod("Period must be > 0".to_string()));
    }

    let multiplier = 2.0 / (period as f64 + 1.0);
    let mut result = Vec::with_capacity(prices.len());

    // First EMA is SMA
    let first_sma: f64 = prices[..period].iter().sum::<f64>() / period as f64;
    result.push(first_sma);

    // Calculate EMA for remaining prices
    for i in period..prices.len() {
        let ema_val = (prices[i] - result.last().unwrap()) * multiplier + result.last().unwrap();
        result.push(ema_val);
    }

    Ok(result)
}

// ============================================================================
// Momentum Indicators
// ============================================================================

/// Relative Strength Index (RSI)
pub fn rsi(prices: &[f64], period: usize) -> Result<Vec<f64>> {
    if prices.len() < period + 1 {
        return Err(IndicatorError::InsufficientData {
            required: period + 1,
            actual: prices.len(),
        });
    }

    if period == 0 {
        return Err(IndicatorError::InvalidPeriod("Period must be > 0".to_string()));
    }

    let mut gains = Vec::new();
    let mut losses = Vec::new();

    // Calculate price changes
    for i in 1..prices.len() {
        let change = prices[i] - prices[i - 1];
        if change > 0.0 {
            gains.push(change);
            losses.push(0.0);
        } else {
            gains.push(0.0);
            losses.push(change.abs());
        }
    }

    let mut result = Vec::new();
    let mut avg_gain = gains[..period].iter().sum::<f64>() / period as f64;
    let mut avg_loss = losses[..period].iter().sum::<f64>() / period as f64;

    // Calculate RSI
    for i in period..gains.len() {
        if avg_loss == 0.0 {
            result.push(100.0);
        } else {
            let rs = avg_gain / avg_loss;
            let rsi_val = 100.0 - (100.0 / (1.0 + rs));
            result.push(rsi_val);
        }

        // Update averages using Wilder's smoothing
        avg_gain = (avg_gain * (period - 1) as f64 + gains[i]) / period as f64;
        avg_loss = (avg_loss * (period - 1) as f64 + losses[i]) / period as f64;
    }

    Ok(result)
}

/// Moving Average Convergence Divergence (MACD)
#[derive(Debug, Clone)]
pub struct MacdResult {
    pub macd_line: Vec<f64>,
    pub signal_line: Vec<f64>,
    pub histogram: Vec<f64>,
}

pub fn macd(prices: &[f64], fast_period: usize, slow_period: usize, signal_period: usize) -> Result<MacdResult> {
    if prices.len() < slow_period {
        return Err(IndicatorError::InsufficientData {
            required: slow_period,
            actual: prices.len(),
        });
    }

    let ema_fast = ema(prices, fast_period)?;
    let ema_slow = ema(prices, slow_period)?;

    // Align the EMAs (slow starts later)
    let offset = slow_period - fast_period;
    let macd_line: Vec<f64> = ema_fast[offset..]
        .iter()
        .zip(ema_slow.iter())
        .map(|(fast, slow)| fast - slow)
        .collect();

    let signal_line = ema(&macd_line, signal_period)?;

    let histogram: Vec<f64> = macd_line[signal_period - 1..]
        .iter()
        .zip(signal_line.iter())
        .map(|(macd, signal)| macd - signal)
        .collect();

    Ok(MacdResult {
        macd_line,
        signal_line,
        histogram,
    })
}

// ============================================================================
// Volatility Indicators
// ============================================================================

/// Bollinger Bands
#[derive(Debug, Clone)]
pub struct BollingerBands {
    pub upper: Vec<f64>,
    pub middle: Vec<f64>,
    pub lower: Vec<f64>,
}

pub fn bollinger_bands(prices: &[f64], period: usize, std_dev: f64) -> Result<BollingerBands> {
    let middle = sma(prices, period)?;

    let mut upper = Vec::with_capacity(middle.len());
    let mut lower = Vec::with_capacity(middle.len());

    for i in period - 1..prices.len() {
        let slice = &prices[i - period + 1..=i];
        let mean = middle[i - period + 1];

        // Calculate standard deviation
        let variance: f64 = slice.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / period as f64;
        let std = variance.sqrt();

        upper.push(mean + std_dev * std);
        lower.push(mean - std_dev * std);
    }

    Ok(BollingerBands {
        upper,
        middle,
        lower,
    })
}

/// Average True Range (ATR)
pub fn atr(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Result<Vec<f64>> {
    if high.len() != low.len() || high.len() != close.len() {
        return Err(IndicatorError::InvalidParameter("Price arrays must have equal length".to_string()));
    }

    if high.len() < period + 1 {
        return Err(IndicatorError::InsufficientData {
            required: period + 1,
            actual: high.len(),
        });
    }

    // Calculate True Range
    let mut tr = Vec::with_capacity(high.len() - 1);
    for i in 1..high.len() {
        let tr1 = high[i] - low[i];
        let tr2 = (high[i] - close[i - 1]).abs();
        let tr3 = (low[i] - close[i - 1]).abs();
        tr.push(tr1.max(tr2).max(tr3));
    }

    // Calculate ATR using Wilder's smoothing
    let mut result = Vec::new();
    let mut atr_val = tr[..period].iter().sum::<f64>() / period as f64;
    result.push(atr_val);

    for i in period..tr.len() {
        atr_val = (atr_val * (period - 1) as f64 + tr[i]) / period as f64;
        result.push(atr_val);
    }

    Ok(result)
}

// ============================================================================
// Volume Indicators
// ============================================================================

/// On-Balance Volume (OBV)
pub fn obv(close: &[f64], volume: &[f64]) -> Result<Vec<f64>> {
    if close.len() != volume.len() {
        return Err(IndicatorError::InvalidParameter("Close and volume must have equal length".to_string()));
    }

    if close.len() < 2 {
        return Err(IndicatorError::InsufficientData {
            required: 2,
            actual: close.len(),
        });
    }

    let mut result = vec![volume[0]];

    for i in 1..close.len() {
        let obv_val = if close[i] > close[i - 1] {
            result.last().unwrap() + volume[i]
        } else if close[i] < close[i - 1] {
            result.last().unwrap() - volume[i]
        } else {
            *result.last().unwrap()
        };
        result.push(obv_val);
    }

    Ok(result)
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Calculate returns from prices
pub fn returns(prices: &[f64]) -> Vec<f64> {
    prices.windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect()
}

/// Calculate log returns from prices
pub fn log_returns(prices: &[f64]) -> Vec<f64> {
    prices.windows(2)
        .map(|w| (w[1] / w[0]).ln())
        .collect()
}

/// Calculate rolling standard deviation
pub fn rolling_std(prices: &[f64], period: usize) -> Result<Vec<f64>> {
    if prices.len() < period {
        return Err(IndicatorError::InsufficientData {
            required: period,
            actual: prices.len(),
        });
    }

    let mut result = Vec::with_capacity(prices.len() - period + 1);

    for i in period - 1..prices.len() {
        let slice = &prices[i - period + 1..=i];
        let mean = slice.iter().sum::<f64>() / period as f64;
        let variance = slice.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / period as f64;
        result.push(variance.sqrt());
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sma() {
        let prices = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = sma(&prices, 3).unwrap();
        assert_eq!(result.len(), 3);
        assert!((result[0] - 2.0).abs() < 1e-10);
        assert!((result[1] - 3.0).abs() < 1e-10);
        assert!((result[2] - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_rsi() {
        let prices = vec![44.0, 44.34, 44.09, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03, 45.61, 46.28, 46.28];
        let result = rsi(&prices, 14).unwrap();
        assert!(result.len() > 0);
        // RSI should be between 0 and 100
        for r in result {
            assert!(r >= 0.0 && r <= 100.0);
        }
    }

    #[test]
    fn test_macd() {
        let prices: Vec<f64> = (0..100).map(|x| 100.0 + (x as f64 * 0.1)).collect();
        let result = macd(&prices, 12, 26, 9).unwrap();
        assert!(result.macd_line.len() > 0);
        assert!(result.signal_line.len() > 0);
        assert!(result.histogram.len() > 0);
    }

    #[test]
    fn test_bollinger_bands() {
        let prices = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let result = bollinger_bands(&prices, 5, 2.0).unwrap();
        assert_eq!(result.upper.len(), result.middle.len());
        assert_eq!(result.middle.len(), result.lower.len());
        // Upper should be above middle, middle above lower
        for i in 0..result.upper.len() {
            assert!(result.upper[i] > result.middle[i]);
            assert!(result.middle[i] > result.lower[i]);
        }
    }

    #[test]
    fn test_returns() {
        let prices = vec![100.0, 110.0, 121.0];
        let result = returns(&prices);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.1).abs() < 1e-10);
        assert!((result[1] - 0.1).abs() < 1e-10);
    }
}
