//! Simulated venue: an order book per symbol, quoted by a market maker whose
//! quotes are replaced on every engine tick.

use orderbook::{
    Order, OrderBook, OrderError, OrderId, Price, Quantity, Side, SubmitResult, Timestamp,
};
use std::collections::BTreeMap;
use thiserror::Error;
use tracing::error;

/// Order ids at or above this value belong to the simulated market maker;
/// engine child order ids count up from 1 and never reach it.
pub const QUOTE_ID_BASE: u64 = 1 << 63;

/// `(price, total quantity)` per level, best price first.
pub type Depth = Vec<(Price, Quantity)>;

/// One simulated market.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolConfig {
    pub symbol: String,
    /// Mid price the market maker quotes around. It never moves: fills consume
    /// depth, and the next refresh restores it.
    pub reference_price: Price,
    /// Price levels quoted on each side.
    pub levels: u32,
    pub qty_per_level: u64,
    /// Gap between levels in basis points of the reference price (at least one tick).
    pub level_spacing_bps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VenueError {
    #[error("unknown symbol {0}")]
    UnknownSymbol(String),
    #[error("invalid market {symbol}: {reason}")]
    InvalidConfig {
        symbol: String,
        reason: &'static str,
    },
    #[error("the {symbol} book rejected the order: {source}")]
    Rejected { symbol: String, source: OrderError },
}

struct Market {
    config: SymbolConfig,
    book: OrderBook,
    /// Distance between quote levels in ticks.
    step: i64,
    quote_ids: Vec<OrderId>,
}

/// Order books for every configured symbol.
pub struct SimulatedVenue {
    markets: BTreeMap<String, Market>,
    next_quote_id: u64,
}

impl SimulatedVenue {
    /// Validates the market configurations and quotes every market.
    pub fn new(symbols: Vec<SymbolConfig>) -> Result<Self, VenueError> {
        let mut markets = BTreeMap::new();
        for config in symbols {
            let step = level_step(&config)?;
            if markets.contains_key(&config.symbol) {
                return Err(VenueError::InvalidConfig {
                    symbol: config.symbol,
                    reason: "duplicate symbol",
                });
            }
            let book = OrderBook::new(config.symbol.clone());
            markets.insert(
                config.symbol.clone(),
                Market {
                    config,
                    book,
                    step,
                    quote_ids: Vec::new(),
                },
            );
        }
        let mut venue = Self {
            markets,
            next_quote_id: QUOTE_ID_BASE,
        };
        venue.refresh_quotes();
        Ok(venue)
    }

    /// Replaces every market maker quote with a full ladder around the reference price.
    pub fn refresh_quotes(&mut self) {
        for market in self.markets.values_mut() {
            for id in market.quote_ids.drain(..) {
                market.book.cancel_order(id);
            }
            let reference = market.config.reference_price.ticks();
            for level in 1..=i64::from(market.config.levels) {
                let offset = market.step * level;
                for (side, ticks) in [
                    (Side::Sell, reference + offset),
                    (Side::Buy, reference - offset),
                ] {
                    let id = OrderId::new(self.next_quote_id);
                    self.next_quote_id += 1;
                    let quote = Order::limit(
                        id,
                        side,
                        Price::new(ticks),
                        Quantity::new(market.config.qty_per_level),
                        Timestamp::new(0),
                    );
                    match market.book.submit_order(quote) {
                        Ok(_) => market.quote_ids.push(id),
                        // Unreachable: configurations are validated and quotes never cross.
                        Err(e) => error!(
                            "failed to quote {} {side} at {ticks} ticks: {e}",
                            market.config.symbol
                        ),
                    }
                }
            }
        }
    }

    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.markets.keys().map(String::as_str)
    }

    pub fn contains(&self, symbol: &str) -> bool {
        self.markets.contains_key(symbol)
    }

    /// Midpoint of the best bid and ask, if both sides are quoted.
    pub fn mid(&self, symbol: &str) -> Option<Price> {
        self.markets.get(symbol)?.book.mid()
    }

    /// The price the market maker quotes around.
    pub fn reference_price(&self, symbol: &str) -> Option<Price> {
        Some(self.markets.get(symbol)?.config.reference_price)
    }

    /// Top `levels` bid and ask levels.
    pub fn depth(&self, symbol: &str, levels: usize) -> Option<(Depth, Depth)> {
        let book = &self.markets.get(symbol)?.book;
        Some((book.bid_levels(levels), book.ask_levels(levels)))
    }

    /// Sends an order to a symbol's book.
    pub fn execute(&mut self, symbol: &str, order: Order) -> Result<SubmitResult, VenueError> {
        let market = self
            .markets
            .get_mut(symbol)
            .ok_or_else(|| VenueError::UnknownSymbol(symbol.to_string()))?;
        market
            .book
            .submit_order(order)
            .map_err(|source| VenueError::Rejected {
                symbol: symbol.to_string(),
                source,
            })
    }
}

/// Checks a market configuration and returns its level spacing in ticks.
fn level_step(config: &SymbolConfig) -> Result<i64, VenueError> {
    let invalid = |reason| {
        Err(VenueError::InvalidConfig {
            symbol: config.symbol.clone(),
            reason,
        })
    };
    if config.symbol.is_empty() {
        return invalid("symbol must not be empty");
    }
    if config.reference_price.ticks() <= 0 {
        return invalid("reference_price must be positive");
    }
    if config.levels == 0 {
        return invalid("levels must be positive");
    }
    if config.qty_per_level == 0 {
        return invalid("qty_per_level must be positive");
    }
    if config.level_spacing_bps == 0 {
        return invalid("level_spacing_bps must be positive");
    }
    let reference = i128::from(config.reference_price.ticks());
    let step = (reference * i128::from(config.level_spacing_bps) / 10_000).max(1);
    let depth = step * i128::from(config.levels);
    if reference - depth <= 0 {
        return invalid("the deepest bid must stay above zero");
    }
    if reference + depth > i128::from(i64::MAX) {
        return invalid("the deepest ask must fit in i64 ticks");
    }
    // step <= reference, so it fits in i64.
    Ok(step as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MID: i64 = 10_000_000;

    fn config(symbol: &str, levels: u32, qty_per_level: u64) -> SymbolConfig {
        SymbolConfig {
            symbol: symbol.to_string(),
            reference_price: Price::new(MID),
            levels,
            qty_per_level,
            level_spacing_bps: 1,
        }
    }

    #[test]
    fn quotes_a_symmetric_ladder_around_the_reference_price() {
        let venue = SimulatedVenue::new(vec![config("SIM", 3, 50)]).unwrap();
        let (bids, asks) = venue.depth("SIM", 10).unwrap();
        let level = |ticks| (Price::new(ticks), Quantity::new(50));
        assert_eq!(
            bids,
            vec![level(MID - 1_000), level(MID - 2_000), level(MID - 3_000)]
        );
        assert_eq!(
            asks,
            vec![level(MID + 1_000), level(MID + 2_000), level(MID + 3_000)]
        );
        assert_eq!(venue.mid("SIM"), Some(Price::new(MID)));
    }

    #[test]
    fn refresh_restores_depth_consumed_by_fills() {
        let mut venue = SimulatedVenue::new(vec![config("SIM", 2, 50)]).unwrap();
        let taker = Order::market(
            OrderId::new(1),
            Side::Buy,
            Quantity::new(70),
            Timestamp::new(0),
        );
        let result = venue.execute("SIM", taker).unwrap();
        assert_eq!(result.filled_qty, Quantity::new(70));
        assert_eq!(
            venue.depth("SIM", 10).unwrap().1,
            vec![(Price::new(MID + 2_000), Quantity::new(30))]
        );

        venue.refresh_quotes();
        assert_eq!(
            venue.depth("SIM", 10).unwrap().1,
            vec![
                (Price::new(MID + 1_000), Quantity::new(50)),
                (Price::new(MID + 2_000), Quantity::new(50))
            ]
        );
    }

    #[test]
    fn level_spacing_is_at_least_one_tick() {
        let tiny = SymbolConfig {
            reference_price: Price::new(100),
            ..config("TINY", 2, 1)
        };
        let venue = SimulatedVenue::new(vec![tiny]).unwrap();
        let (bids, asks) = venue.depth("TINY", 10).unwrap();
        let ticks = |levels: &Depth| levels.iter().map(|(p, _)| p.ticks()).collect::<Vec<_>>();
        assert_eq!(ticks(&asks), vec![101, 102]);
        assert_eq!(ticks(&bids), vec![99, 98]);
    }

    #[test]
    fn invalid_configurations_are_rejected() {
        let rejected = |configs: Vec<SymbolConfig>| {
            matches!(
                SimulatedVenue::new(configs),
                Err(VenueError::InvalidConfig { .. })
            )
        };
        assert!(rejected(vec![config("", 1, 1)]));
        assert!(rejected(vec![SymbolConfig {
            reference_price: Price::new(0),
            ..config("A", 1, 1)
        }]));
        assert!(rejected(vec![config("A", 0, 1)]));
        assert!(rejected(vec![config("A", 1, 0)]));
        assert!(rejected(vec![SymbolConfig {
            level_spacing_bps: 0,
            ..config("A", 1, 1)
        }]));
        // Ten levels one tick apart around 10 ticks would put the deepest bid at zero.
        assert!(rejected(vec![SymbolConfig {
            reference_price: Price::new(10),
            ..config("A", 10, 1)
        }]));
        assert!(rejected(vec![config("A", 1, 1), config("A", 1, 1)]));
    }

    #[test]
    fn unknown_symbols_are_errors() {
        let mut venue = SimulatedVenue::new(vec![config("SIM", 1, 1)]).unwrap();
        let order = Order::market(
            OrderId::new(1),
            Side::Buy,
            Quantity::new(1),
            Timestamp::new(0),
        );
        assert_eq!(
            venue.execute("NOPE", order).unwrap_err(),
            VenueError::UnknownSymbol("NOPE".to_string())
        );
        assert_eq!(venue.mid("NOPE"), None);
        assert!(!venue.contains("NOPE"));
    }
}
