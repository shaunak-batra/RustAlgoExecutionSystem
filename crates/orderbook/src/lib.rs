//! # Order Book
//!
//! Single-symbol limit order book with price-time priority matching.
//!
//! - **Price-Time Priority Matching**: FIFO order matching at each price level
//! - **Time-in-Force**: GTC, IOC (Immediate-or-Cancel), FOK (Fill-or-Kill)
//!
//! ## Key Components
//!
//! - [`OrderBook`]: Main order book with bid/ask level tracking
//! - [`Order`]: Order representation with price, quantity, and time-in-force
//! - [`Fill`]: Execution record with price, quantity, and timestamp
//! - [`Price`], [`Quantity`], [`Timestamp`]: Type-safe wrappers for financial data
//!
//! ## Example Usage
//!
//! ```
//! use orderbook::{OrderBook, Order, OrderId, Price, Quantity, Side, TimeInForce, Timestamp};
//!
//! let mut book = OrderBook::new("BTC-USD".to_string());
//!
//! // Submit a limit buy order
//! let order = Order {
//!     id: OrderId::new(1),
//!     side: Side::Buy,
//!     price: Price::from_f64(100.0),
//!     qty: Quantity::new(10),
//!     timestamp: Timestamp::now_nanos(),
//!     tif: TimeInForce::GTC,
//! };
//!
//! let fills = book.insert_order(order);
//! println!("Fills: {:?}", fills);
//! ```

pub mod book;
pub mod types;

pub use book::*;
pub use types::*;
