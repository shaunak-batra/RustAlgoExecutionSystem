use crate::types::{Quote, Trade, MarketSnapshot};
use orderbook::{Price, Timestamp};
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};

pub struct MarketDataFeed {
    symbol: String,
    base_price: f64,
    quote_tx: broadcast::Sender<Quote>,
    trade_tx: broadcast::Sender<Trade>,
    snapshot_tx: broadcast::Sender<MarketSnapshot>,
    shutdown_tx: broadcast::Sender<()>,
}

impl MarketDataFeed {
    pub fn new(symbol: String, base_price: f64) -> Self {
        let (quote_tx, _) = broadcast::channel(1000);
        let (trade_tx, _) = broadcast::channel(1000);
        let (snapshot_tx, _) = broadcast::channel(100);
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            symbol,
            base_price,
            quote_tx,
            trade_tx,
            snapshot_tx,
            shutdown_tx,
        }
    }

    /// Gracefully shut down the market data feed
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
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

    pub async fn start(&self) {
        let symbol = self.symbol.clone();
        let base_price = self.base_price;
        let quote_tx = self.quote_tx.clone();
        let trade_tx = self.trade_tx.clone();
        let snapshot_tx = self.snapshot_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let mut quote_interval = interval(Duration::from_millis(100));
            let mut trade_interval = interval(Duration::from_millis(500));
            let mut snapshot_interval = interval(Duration::from_secs(1));

            let mut rng = StdRng::from_entropy();
            let mut current_price = base_price;
            let mut cumulative_volume = 0u64;
            let mut vwap_sum = 0.0;

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        println!("Market data feed shutting down gracefully for symbol: {}", symbol);
                        break;
                    }

                    _ = quote_interval.tick() => {
                        let drift = rng.gen_range(-0.1..0.1);
                        current_price += drift;
                        current_price = current_price.max(base_price * 0.95).min(base_price * 1.05);

                        let spread = current_price * 0.0001;
                        let bid = current_price - spread / 2.0;
                        let ask = current_price + spread / 2.0;

                        let quote = Quote {
                            symbol: symbol.clone(),
                            bid: Price::from_f64(bid),
                            ask: Price::from_f64(ask),
                            bid_size: rng.gen_range(100..1000),
                            ask_size: rng.gen_range(100..1000),
                            timestamp_ns: Timestamp::now_nanos().nanos(),
                        };

                        let _ = quote_tx.send(quote);
                    }

                    _ = trade_interval.tick() => {
                        let size = rng.gen_range(10..200);
                        cumulative_volume += size;
                        vwap_sum += current_price * size as f64;

                        let trade = Trade {
                            symbol: symbol.clone(),
                            price: Price::from_f64(current_price),
                            size,
                            timestamp_ns: Timestamp::now_nanos().nanos(),
                        };

                        let _ = trade_tx.send(trade);
                    }

                    _ = snapshot_interval.tick() => {
                        let vwap = if cumulative_volume > 0 {
                            vwap_sum / cumulative_volume as f64
                        } else {
                            current_price
                        };

                        let snapshot = MarketSnapshot {
                            symbol: symbol.clone(),
                            last_price: Price::from_f64(current_price),
                            volume: cumulative_volume,
                            vwap,
                            timestamp_ns: Timestamp::now_nanos().nanos(),
                        };

                        let _ = snapshot_tx.send(snapshot);
                    }
                }
            }
        });
    }
}
