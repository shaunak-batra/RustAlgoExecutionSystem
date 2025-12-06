#![allow(dead_code, unused_variables)]

use crate::types::*;
use serde::{Deserialize, Serialize};

/// Market impact model configuration
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MarketImpactConfig {
    /// Risk aversion parameter (lambda in Almgren-Chriss)
    pub risk_aversion: f64,
    /// Permanent impact coefficient
    pub permanent_impact: f64,
    /// Temporary impact coefficient
    pub temporary_impact: f64,
    /// Volatility (annualized)
    pub volatility: f64,
}

impl Default for MarketImpactConfig {
    fn default() -> Self {
        Self {
            risk_aversion: 1.0,
            permanent_impact: 0.1,
            temporary_impact: 0.1,
            volatility: 0.3,
        }
    }
}

/// Almgren-Chriss market impact model
pub struct AlmgrenChrissModel {
    config: MarketImpactConfig,
}

impl AlmgrenChrissModel {
    pub fn new(config: MarketImpactConfig) -> Self {
        Self { config }
    }

    /// Calculate permanent price impact
    /// Permanent impact: g(v) = γ * v where v is trade rate
    pub fn permanent_impact(&self, trade_qty: f64, total_volume: f64) -> f64 {
        let participation_rate = if total_volume > 0.0 {
            trade_qty / total_volume
        } else {
            0.0
        };
        self.config.permanent_impact * participation_rate
    }

    /// Calculate temporary price impact
    /// Temporary impact: h(v) = ε * sign(v) * v
    pub fn temporary_impact(&self, trade_qty: f64, total_volume: f64) -> f64 {
        let participation_rate = if total_volume > 0.0 {
            trade_qty / total_volume
        } else {
            0.0
        };
        self.config.temporary_impact * participation_rate
    }

    /// Calculate execution cost for a trade
    /// Total cost = permanent_impact + temporary_impact
    pub fn execution_cost(
        &self,
        current_price: f64,
        trade_qty: f64,
        total_volume: f64,
        side: Side,
    ) -> f64 {
        let perm = self.permanent_impact(trade_qty.abs(), total_volume);
        let temp = self.temporary_impact(trade_qty.abs(), total_volume);

        let total_impact = perm + temp;

        // Impact increases price for buys, decreases for sells
        match side {
            Side::Buy => current_price * (1.0 + total_impact),
            Side::Sell => current_price * (1.0 - total_impact),
        }
    }

    /// Calculate optimal execution strategy (TWAP slicing)
    pub fn optimal_trajectory(
        &self,
        total_qty: f64,
        duration_secs: f64,
        num_slices: usize,
    ) -> Vec<f64> {
        // For Almgren-Chriss with linear impact, optimal is approximately TWAP
        let qty_per_slice = total_qty / num_slices as f64;
        vec![qty_per_slice; num_slices]
    }
}

/// Dynamic spread model
#[derive(Debug, Clone)]
pub struct DynamicSpreadModel {
    base_spread_bps: f64,
    volatility_multiplier: f64,
    volume_adjustment: f64,
}

impl DynamicSpreadModel {
    pub fn new(base_spread_bps: f64) -> Self {
        Self {
            base_spread_bps,
            volatility_multiplier: 1.5,
            volume_adjustment: 0.1,
        }
    }

    /// Calculate dynamic spread based on market conditions
    pub fn calculate_spread(
        &self,
        mid_price: f64,
        volatility: f64,
        volume: f64,
        avg_volume: f64,
    ) -> f64 {
        // Base spread
        let base = mid_price * (self.base_spread_bps / 10000.0);

        // Volatility adjustment: higher vol = wider spread
        let vol_adjustment = base * (volatility / 0.3) * self.volatility_multiplier;

        // Volume adjustment: lower volume = wider spread
        let volume_ratio = if avg_volume > 0.0 {
            volume / avg_volume
        } else {
            1.0
        };
        let volume_adj = base * self.volume_adjustment * (1.0 / volume_ratio.max(0.1));

        (base + vol_adjustment + volume_adj).max(mid_price * 0.0001) // Min 1 bps
    }

    /// Calculate bid/ask prices from mid and spread
    pub fn calculate_quotes(&self, mid_price: f64, spread: f64) -> (f64, f64) {
        let half_spread = spread / 2.0;
        let bid = mid_price - half_spread;
        let ask = mid_price + half_spread;
        (bid, ask)
    }
}

/// Liquidity depth model
#[derive(Debug, Clone)]
pub struct LiquidityDepthModel {
    levels: Vec<LiquidityLevel>,
    decay_factor: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct LiquidityLevel {
    pub distance_bps: f64,  // Distance from mid in basis points
    pub quantity_pct: f64,  // Percentage of total liquidity
}

impl LiquidityDepthModel {
    /// Create a realistic depth profile
    pub fn new_exponential(total_liquidity: f64, decay_factor: f64) -> Self {
        let mut levels = Vec::new();

        // Generate levels from 0 to 100 bps away from mid
        for i in 0..20 {
            let distance = (i as f64) * 5.0; // 0, 5, 10, ... 95 bps
            let quantity_pct = (-decay_factor * (i as f64)).exp();
            levels.push(LiquidityLevel {
                distance_bps: distance,
                quantity_pct,
            });
        }

        // Normalize so percentages sum to ~1.0
        let total: f64 = levels.iter().map(|l| l.quantity_pct).sum();
        for level in &mut levels {
            level.quantity_pct = (level.quantity_pct / total) * total_liquidity;
        }

        Self {
            levels,
            decay_factor,
        }
    }

    /// Get available liquidity at a specific price level
    pub fn liquidity_at_price(&self, mid_price: f64, price: f64) -> f64 {
        let distance_bps = ((price - mid_price).abs() / mid_price) * 10000.0;

        // Find closest level
        self.levels
            .iter()
            .min_by(|a, b| {
                let dist_a = (a.distance_bps - distance_bps).abs();
                let dist_b = (b.distance_bps - distance_bps).abs();
                dist_a.partial_cmp(&dist_b).unwrap()
            })
            .map(|l| l.quantity_pct)
            .unwrap_or(0.0)
    }

    /// Calculate partial fill based on available liquidity
    pub fn calculate_partial_fill(
        &self,
        mid_price: f64,
        order_price: f64,
        order_qty: f64,
    ) -> f64 {
        let available = self.liquidity_at_price(mid_price, order_price);
        order_qty.min(available)
    }

    /// Get all levels for visualization
    pub fn get_levels(&self) -> &[LiquidityLevel] {
        &self.levels
    }
}

/// Comprehensive market simulator
pub struct MarketSimulator {
    impact_model: AlmgrenChrissModel,
    spread_model: DynamicSpreadModel,
    liquidity_model: LiquidityDepthModel,
    current_mid: f64,
    current_volume: f64,
    avg_volume: f64,
    volatility: f64,
}

impl MarketSimulator {
    pub fn new(
        initial_mid: f64,
        avg_volume: f64,
        volatility: f64,
        impact_config: MarketImpactConfig,
    ) -> Self {
        Self {
            impact_model: AlmgrenChrissModel::new(impact_config),
            spread_model: DynamicSpreadModel::new(5.0), // 5 bps base spread
            liquidity_model: LiquidityDepthModel::new_exponential(avg_volume * 0.1, 0.1),
            current_mid: initial_mid,
            current_volume: avg_volume,
            avg_volume,
            volatility,
        }
    }

    /// Update market state
    pub fn update(&mut self, mid_price: f64, volume: f64, volatility: f64) {
        self.current_mid = mid_price;
        self.current_volume = volume;
        self.volatility = volatility;
    }

    /// Simulate order execution with realistic impact and partial fills
    pub fn simulate_execution(
        &self,
        order_price: f64,
        order_qty: f64,
        side: Side,
    ) -> ExecutionResult {
        // Calculate market impact
        let impacted_price = self.impact_model.execution_cost(
            self.current_mid,
            order_qty,
            self.current_volume,
            side,
        );

        // Calculate dynamic spread
        let spread = self.spread_model.calculate_spread(
            self.current_mid,
            self.volatility,
            self.current_volume,
            self.avg_volume,
        );
        let (bid, ask) = self.spread_model.calculate_quotes(self.current_mid, spread);

        // Check if order crosses spread (marketable)
        let is_marketable = match side {
            Side::Buy => order_price >= ask,
            Side::Sell => order_price <= bid,
        };

        // Calculate partial fill based on liquidity
        let available_liquidity = self.liquidity_model.calculate_partial_fill(
            self.current_mid,
            order_price,
            order_qty,
        );

        let fill_qty = if is_marketable {
            order_qty.min(available_liquidity)
        } else {
            0.0 // Order doesn't cross, no immediate fill
        };

        let fill_price = if fill_qty > 0.0 {
            impacted_price
        } else {
            order_price
        };

        ExecutionResult {
            filled_qty: fill_qty,
            remaining_qty: order_qty - fill_qty,
            fill_price,
            market_impact: impacted_price - self.current_mid,
            spread,
            bid,
            ask,
        }
    }

    pub fn get_current_quotes(&self) -> (f64, f64, f64) {
        let spread = self.spread_model.calculate_spread(
            self.current_mid,
            self.volatility,
            self.current_volume,
            self.avg_volume,
        );
        let (bid, ask) = self.spread_model.calculate_quotes(self.current_mid, spread);
        (bid, self.current_mid, ask)
    }
}

/// Execution simulation result
#[derive(Debug, Clone, Copy)]
pub struct ExecutionResult {
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub fill_price: f64,
    pub market_impact: f64,
    pub spread: f64,
    pub bid: f64,
    pub ask: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_almgren_chriss_impact() {
        let config = MarketImpactConfig::default();
        let model = AlmgrenChrissModel::new(config);

        let price = model.execution_cost(50000.0, 100.0, 10000.0, Side::Buy);
        assert!(price > 50000.0); // Buy should increase price

        let price = model.execution_cost(50000.0, 100.0, 10000.0, Side::Sell);
        assert!(price < 50000.0); // Sell should decrease price
    }

    #[test]
    fn test_dynamic_spread() {
        let model = DynamicSpreadModel::new(5.0);

        // Low volatility = tight spread
        let spread1 = model.calculate_spread(50000.0, 0.1, 10000.0, 10000.0);

        // High volatility = wide spread
        let spread2 = model.calculate_spread(50000.0, 0.5, 10000.0, 10000.0);

        assert!(spread2 > spread1);
    }

    #[test]
    fn test_liquidity_depth() {
        let model = LiquidityDepthModel::new_exponential(10000.0, 0.1);

        let levels = model.get_levels();
        assert!(!levels.is_empty());

        // Liquidity should decay with distance
        assert!(levels[0].quantity_pct > levels[levels.len() - 1].quantity_pct);
    }

    #[test]
    fn test_market_simulator() {
        let config = MarketImpactConfig::default();
        let sim = MarketSimulator::new(50000.0, 10000.0, 0.3, config);

        // Marketable buy order
        let result = sim.simulate_execution(51000.0, 100.0, Side::Buy);
        assert!(result.filled_qty > 0.0);
        assert!(result.fill_price >= sim.current_mid);

        // Non-marketable limit order
        let result = sim.simulate_execution(49000.0, 100.0, Side::Buy);
        assert_eq!(result.filled_qty, 0.0);
    }
}
