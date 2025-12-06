use orderbook::{Order, OrderId, Price, Quantity, Side, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// Constants
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Adaptive TWAP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveTwapConfig {
    /// Target total quantity to execute
    pub total_qty: u64,
    /// Total execution duration in seconds
    pub duration_secs: u64,
    /// Base slice duration in seconds
    pub base_slice_secs: u64,
    /// Enable adaptive slicing based on volatility
    pub enable_adaptive: bool,
    /// Volatility calculation window (number of price observations)
    pub volatility_window: usize,
    /// Participation rate limit (0.0 to 1.0)
    pub max_participation: f64,
}

impl Default for AdaptiveTwapConfig {
    fn default() -> Self {
        Self {
            total_qty: 1000,
            duration_secs: 300,
            base_slice_secs: 60,
            enable_adaptive: true,
            volatility_window: 20,
            max_participation: 0.3,
        }
    }
}

/// Adaptive TWAP executor
pub struct AdaptiveTwap {
    config: AdaptiveTwapConfig,
    #[allow(dead_code)]
    symbol: String,
    side: Side,
    start_time: u64,
    end_time: u64,
    filled_qty: u64,
    price_history: VecDeque<f64>,
    current_slice: usize,
    total_slices: usize,
}

impl AdaptiveTwap {
    pub fn new(config: AdaptiveTwapConfig, symbol: String, side: Side, start_time: u64) -> Self {
        let total_slices = (config.duration_secs / config.base_slice_secs) as usize;
        let end_time = start_time + (config.duration_secs * NANOS_PER_SECOND);
        let volatility_window = config.volatility_window;

        Self {
            config,
            symbol,
            side,
            start_time,
            end_time,
            filled_qty: 0,
            price_history: VecDeque::with_capacity(volatility_window),
            current_slice: 0,
            total_slices,
        }
    }

    /// Update price history for volatility calculation
    pub fn update_price(&mut self, price: f64) {
        self.price_history.push_back(price);
        if self.price_history.len() > self.config.volatility_window {
            self.price_history.pop_front();
        }
    }

    /// Calculate historical volatility (standard deviation of returns)
    fn calculate_volatility(&self) -> f64 {
        if self.price_history.len() < 2 {
            return 0.01; // Default low volatility
        }

        // Calculate returns
        let returns: Vec<f64> = self
            .price_history
            .iter()
            .zip(self.price_history.iter().skip(1))
            .map(|(p1, p2)| (p2 / p1).ln())
            .collect();

        if returns.is_empty() {
            return 0.01;
        }

        // Calculate mean
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;

        // Calculate variance
        let variance = returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / returns.len() as f64;

        variance.sqrt()
    }

    /// Calculate adaptive slice quantity based on market conditions
    fn calculate_adaptive_slice_qty(&self, market_volume: f64) -> u64 {
        let remaining_qty = self.config.total_qty - self.filled_qty;
        let remaining_slices = self.total_slices - self.current_slice;

        if remaining_slices == 0 {
            return remaining_qty;
        }

        if !self.config.enable_adaptive {
            // Standard TWAP: equal slices
            return remaining_qty / remaining_slices as u64;
        }

        // Adaptive logic: adjust based on volatility and volume
        let volatility = self.calculate_volatility();
        let base_qty = remaining_qty / remaining_slices as u64;

        // Higher volatility = smaller slices (more cautious)
        let vol_adjustment = if volatility > 0.02 {
            0.8 // Reduce slice by 20% in high volatility
        } else if volatility > 0.01 {
            0.9 // Reduce by 10% in moderate volatility
        } else {
            1.0 // Normal volatility
        };

        // Volume participation constraint
        let max_participation_qty = (market_volume * self.config.max_participation) as u64;

        let adjusted_qty = (base_qty as f64 * vol_adjustment) as u64;
        adjusted_qty.min(max_participation_qty).max(1)
    }

    /// Generate next child order
    pub fn next_order(
        &mut self,
        current_time: u64,
        current_price: Price,
        market_volume: f64,
        order_id_gen: &mut dyn FnMut() -> OrderId,
    ) -> Option<Order> {
        // Check if execution is complete
        if self.filled_qty >= self.config.total_qty {
            return None;
        }

        // Check if execution time has expired
        if current_time > self.end_time {
            // Send remaining quantity as final order
            let remaining = self.config.total_qty - self.filled_qty;
            if remaining > 0 {
                return Some(Order {
                    id: order_id_gen(),
                    side: self.side,
                    price: current_price,
                    qty: Quantity::new(remaining),
                    timestamp: Timestamp::new(current_time),
                    tif: orderbook::TimeInForce::IOC, // IOC for final sweep
                });
            }
            return None;
        }

        // Calculate slice quantity
        let slice_qty = self.calculate_adaptive_slice_qty(market_volume);

        if slice_qty == 0 {
            return None;
        }

        // Create child order
        let order = Order {
            id: order_id_gen(),
            side: self.side,
            price: current_price,
            qty: Quantity::new(slice_qty),
            timestamp: Timestamp::new(current_time),
            tif: orderbook::TimeInForce::GTC,
        };

        self.current_slice += 1;

        Some(order)
    }

    /// Record filled quantity
    pub fn record_fill(&mut self, qty: u64) {
        self.filled_qty += qty;
    }

    /// Get execution progress
    pub fn progress(&self) -> f64 {
        if self.config.total_qty == 0 {
            return 100.0;
        }
        (self.filled_qty as f64 / self.config.total_qty as f64) * 100.0
    }

    /// Get time progress
    pub fn time_progress(&self, current_time: u64) -> f64 {
        if current_time < self.start_time {
            return 0.0;
        }
        if current_time >= self.end_time {
            return 100.0;
        }
        let elapsed = current_time - self.start_time;
        let duration = self.end_time - self.start_time;
        (elapsed as f64 / duration as f64) * 100.0
    }

    /// Check if execution is complete
    pub fn is_complete(&self) -> bool {
        self.filled_qty >= self.config.total_qty
    }

    /// Get current volatility reading
    pub fn current_volatility(&self) -> f64 {
        self.calculate_volatility()
    }

    /// Get statistics
    pub fn get_stats(&self) -> AdaptiveTwapStats {
        AdaptiveTwapStats {
            total_qty: self.config.total_qty,
            filled_qty: self.filled_qty,
            remaining_qty: self.config.total_qty - self.filled_qty,
            current_slice: self.current_slice,
            total_slices: self.total_slices,
            volatility: self.calculate_volatility(),
            progress_pct: self.progress(),
        }
    }
}

/// Adaptive TWAP execution statistics
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveTwapStats {
    pub total_qty: u64,
    pub filled_qty: u64,
    pub remaining_qty: u64,
    pub current_slice: usize,
    pub total_slices: usize,
    pub volatility: f64,
    pub progress_pct: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_order_id_gen() -> impl FnMut() -> OrderId {
        let mut counter = 0u64;
        move || {
            counter += 1;
            OrderId::new(counter)
        }
    }

    #[test]
    fn test_adaptive_twap_creation() {
        let config = AdaptiveTwapConfig {
            total_qty: 1000,
            duration_secs: 300,
            base_slice_secs: 60,
            enable_adaptive: true,
            volatility_window: 20,
            max_participation: 0.3,
        };

        let twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        assert_eq!(twap.total_slices, 5); // 300/60 = 5 slices
        assert_eq!(twap.filled_qty, 0);
    }

    #[test]
    fn test_volatility_calculation() {
        let config = AdaptiveTwapConfig::default();
        let mut twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        // Add price history
        for i in 0..20 {
            twap.update_price(50000.0 + (i as f64) * 10.0);
        }

        let vol = twap.calculate_volatility();
        assert!(vol > 0.0);
    }

    #[test]
    fn test_adaptive_slicing() {
        let config = AdaptiveTwapConfig {
            total_qty: 1000,
            duration_secs: 300,
            base_slice_secs: 60,
            enable_adaptive: true,
            volatility_window: 20,
            max_participation: 0.3,
        };

        let mut twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        // Simulate low volatility
        for i in 0..20 {
            twap.update_price(50000.0 + (i as f64) * 0.1);
        }

        let mut gen = create_order_id_gen();
        let order = twap.next_order(1000000, Price::from_f64(50000.0), 10000.0, &mut gen);

        assert!(order.is_some());
        let order = order.unwrap();
        assert_eq!(order.qty.value(), 200); // 1000/5 = 200 per slice
    }

    #[test]
    fn test_participation_rate_limit() {
        let config = AdaptiveTwapConfig {
            total_qty: 10000,
            duration_secs: 300,
            base_slice_secs: 60,
            enable_adaptive: true,
            volatility_window: 20,
            max_participation: 0.1, // 10% max
        };

        let mut twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        let mut gen = create_order_id_gen();
        let order = twap.next_order(1000000, Price::from_f64(50000.0), 1000.0, &mut gen);

        assert!(order.is_some());
        let order = order.unwrap();
        // Should be limited by 10% of 1000 = 100, not 10000/5 = 2000
        assert!(order.qty.value() <= 100);
    }

    #[test]
    fn test_execution_completion() {
        let config = AdaptiveTwapConfig {
            total_qty: 500,
            duration_secs: 300,
            base_slice_secs: 60,
            enable_adaptive: false,
            volatility_window: 20,
            max_participation: 0.5,
        };

        let mut twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        // Execute all slices
        let mut gen = create_order_id_gen();
        for _ in 0..5 {
            if let Some(order) = twap.next_order(1000000, Price::from_f64(50000.0), 10000.0, &mut gen) {
                twap.record_fill(order.qty.value());
            }
        }

        assert!(twap.is_complete());
        assert_eq!(twap.progress(), 100.0);
    }

    #[test]
    fn test_stats() {
        let config = AdaptiveTwapConfig::default();
        let mut twap = AdaptiveTwap::new(config, "BTCUSDT".to_string(), Side::Buy, 1000000);

        twap.record_fill(250);

        let stats = twap.get_stats();
        assert_eq!(stats.filled_qty, 250);
        assert_eq!(stats.remaining_qty, 750);
        assert_eq!(stats.progress_pct, 25.0);
    }
}
