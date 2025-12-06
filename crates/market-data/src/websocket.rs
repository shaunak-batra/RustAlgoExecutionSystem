use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use orderbook::Price;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::types::{Quote, Trade, MarketSnapshot};

/// Binance WebSocket message for 24hr ticker
#[derive(Debug, Clone, Deserialize)]
struct BinanceTicker {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "b")]
    bid_price: String,
    #[serde(rename = "a")]
    ask_price: String,
    #[serde(rename = "B")]
    bid_qty: String,
    #[serde(rename = "A")]
    ask_qty: String,
    #[serde(rename = "c")]
    last_price: String,
    #[serde(rename = "v")]
    volume_24h: String,
}

/// Market data mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketDataMode {
    WebSocket,
    RestApi,
}

/// WebSocket feed configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    pub websocket_url: String,
    pub fallback_on_failure: bool,
    pub reconnect_attempts: u32,
    pub reconnect_delay_ms: u64,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            websocket_url: "wss://stream.binance.com:9443/ws".to_string(),
            fallback_on_failure: true,
            reconnect_attempts: 5,
            reconnect_delay_ms: 1000,
        }
    }
}

/// WebSocket-based market data feed
pub struct WebSocketFeed {
    config: WebSocketConfig,
    symbols: Vec<String>,
    quote_tx: broadcast::Sender<Quote>,
    trade_tx: broadcast::Sender<Trade>,
    snapshot_tx: broadcast::Sender<MarketSnapshot>,
    mode_tx: broadcast::Sender<MarketDataMode>,
}

impl WebSocketFeed {
    pub fn new(config: WebSocketConfig, symbols: Vec<String>) -> Self {
        let (quote_tx, _) = broadcast::channel(1000);
        let (trade_tx, _) = broadcast::channel(1000);
        let (snapshot_tx, _) = broadcast::channel(100);
        let (mode_tx, _) = broadcast::channel(10);

        Self {
            config,
            symbols,
            quote_tx,
            trade_tx,
            snapshot_tx,
            mode_tx,
        }
    }

    pub fn subscribe_quotes(&self) -> broadcast::Receiver<Quote> {
        self.quote_tx.subscribe()
    }

    pub fn subscribe_trades(&self) -> broadcast::Receiver<Trade> {
        self.trade_tx.subscribe()
    }

    pub fn subscribe_snapshots(&self) -> broadcast::Receiver<MarketSnapshot> {
        self.snapshot_tx.subscribe()
    }

    pub fn subscribe_mode_changes(&self) -> broadcast::Receiver<MarketDataMode> {
        self.mode_tx.subscribe()
    }

    /// Start the WebSocket feed with automatic reconnection and fallback
    pub async fn start(&self) {
        let config = self.config.clone();
        let symbols = self.symbols.clone();
        let quote_tx = self.quote_tx.clone();
        let snapshot_tx = self.snapshot_tx.clone();
        let mode_tx = self.mode_tx.clone();

        tokio::spawn(async move {
            let mut reconnect_count = 0;

            loop {
                info!("Attempting to connect to WebSocket (attempt {}/{})",
                      reconnect_count + 1, config.reconnect_attempts);

                match Self::connect_and_run(
                    &config,
                    &symbols,
                    &quote_tx,
                    &snapshot_tx,
                ).await {
                    Ok(_) => {
                        info!("WebSocket connection closed normally");
                        reconnect_count = 0;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        reconnect_count += 1;

                        if reconnect_count >= config.reconnect_attempts {
                            error!("Max reconnection attempts reached, falling back to REST API mode");

                            if config.fallback_on_failure {
                                let _ = mode_tx.send(MarketDataMode::RestApi);
                                return; // Exit the WebSocket task
                            }
                        }
                    }
                }

                // Wait before reconnecting
                warn!("Waiting {}ms before reconnecting...", config.reconnect_delay_ms);
                sleep(Duration::from_millis(config.reconnect_delay_ms)).await;
            }
        });
    }

    /// Connect to WebSocket and process messages
    async fn connect_and_run(
        config: &WebSocketConfig,
        symbols: &[String],
        quote_tx: &broadcast::Sender<Quote>,
        snapshot_tx: &broadcast::Sender<MarketSnapshot>,
    ) -> Result<()> {
        // Build WebSocket URL for multiple ticker streams
        let streams: Vec<String> = symbols
            .iter()
            .map(|s| format!("{}@ticker", s.to_lowercase()))
            .collect();
        let stream_names = streams.join("/");
        let ws_url = format!("{}/stream?streams={}", config.websocket_url, stream_names);

        info!("Connecting to: {}", ws_url);

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("WebSocket connected successfully");

        let (mut write, mut read) = ws_stream.split();

        // Send ping every 30 seconds to keep connection alive
        let ping_task = tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(30)).await;
                if write.send(Message::Ping(vec![])).await.is_err() {
                    break;
                }
            }
        });

        // Process incoming messages
        while let Some(msg_result) = read.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    if let Err(e) = Self::process_message(&text, quote_tx, snapshot_tx) {
                        warn!("Failed to process message: {}", e);
                    }
                }
                Ok(Message::Binary(_)) => {
                    // Binance sends text messages, not binary
                }
                Ok(Message::Ping(_)) => {
                    // Handled automatically by tungstenite
                }
                Ok(Message::Pong(_)) => {
                    // Response to our ping
                }
                Ok(Message::Close(_)) => {
                    info!("Received close frame from server");
                    break;
                }
                Ok(Message::Frame(_)) => {
                    // Raw frame, shouldn't happen
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
            }
        }

        ping_task.abort();
        Ok(())
    }

    /// Process a WebSocket message
    fn process_message(
        text: &str,
        quote_tx: &broadcast::Sender<Quote>,
        snapshot_tx: &broadcast::Sender<MarketSnapshot>,
    ) -> Result<()> {
        // Binance wraps ticker data in a "data" field when using /stream endpoint
        #[derive(Deserialize)]
        struct StreamWrapper {
            data: BinanceTicker,
        }

        let wrapper: StreamWrapper = serde_json::from_str(text)
            .context("Failed to parse WebSocket message")?;

        let ticker = wrapper.data;
        let timestamp_ns = orderbook::Timestamp::now_nanos().nanos();

        // Parse prices and quantities
        let bid_price = ticker.bid_price.parse::<f64>()
            .context("Invalid bid price")?;
        let ask_price = ticker.ask_price.parse::<f64>()
            .context("Invalid ask price")?;
        let bid_qty = ticker.bid_qty.parse::<f64>()
            .context("Invalid bid quantity")? as u64;
        let ask_qty = ticker.ask_qty.parse::<f64>()
            .context("Invalid ask quantity")? as u64;
        let last_price = ticker.last_price.parse::<f64>()
            .context("Invalid last price")?;
        let volume_24h = ticker.volume_24h.parse::<f64>()
            .context("Invalid 24h volume")?;

        // Send quote update
        let quote = Quote {
            symbol: ticker.symbol.clone(),
            bid: Price::from_f64(bid_price),
            ask: Price::from_f64(ask_price),
            bid_size: bid_qty,
            ask_size: ask_qty,
            timestamp_ns,
        };
        let _ = quote_tx.send(quote);

        // Send market snapshot (less frequent)
        let snapshot = MarketSnapshot {
            symbol: ticker.symbol,
            last_price: Price::from_f64(last_price),
            volume: volume_24h as u64,
            vwap: last_price, // Binance doesn't provide VWAP directly
            timestamp_ns,
        };
        let _ = snapshot_tx.send(snapshot);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_config_default() {
        let config = WebSocketConfig::default();
        assert_eq!(config.websocket_url, "wss://stream.binance.com:9443/ws");
        assert_eq!(config.fallback_on_failure, true);
        assert_eq!(config.reconnect_attempts, 5);
        assert_eq!(config.reconnect_delay_ms, 1000);
    }

    #[test]
    fn test_parse_binance_ticker() {
        let json = r#"{
            "data": {
                "s": "BTCUSDT",
                "b": "50000.00",
                "a": "50001.00",
                "B": "100.0",
                "A": "150.0",
                "c": "50000.50",
                "v": "1000000.0"
            }
        }"#;

        #[derive(Deserialize)]
        struct StreamWrapper {
            data: BinanceTicker,
        }

        let wrapper: StreamWrapper = serde_json::from_str(json).unwrap();
        let ticker = wrapper.data;

        assert_eq!(ticker.symbol, "BTCUSDT");
        assert_eq!(ticker.bid_price, "50000.00");
        assert_eq!(ticker.ask_price, "50001.00");
    }

    #[tokio::test]
    async fn test_websocket_feed_creation() {
        let config = WebSocketConfig::default();
        let symbols = vec!["btcusdt".to_string(), "ethusdt".to_string()];
        let feed = WebSocketFeed::new(config, symbols);

        // Test that we can subscribe to channels
        let _quote_rx = feed.subscribe_quotes();
        let _trade_rx = feed.subscribe_trades();
        let _snapshot_rx = feed.subscribe_snapshots();
        let _mode_rx = feed.subscribe_mode_changes();
    }
}
