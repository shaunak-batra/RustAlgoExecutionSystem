use orderbook::Price;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub bid: Price,
    pub ask: Price,
    pub bid_size: u64,
    pub ask_size: u64,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub symbol: String,
    pub price: Price,
    pub size: u64,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub symbol: String,
    pub last_price: Price,
    pub volume: u64,
    pub vwap: f64,
    pub timestamp_ns: u64,
}
