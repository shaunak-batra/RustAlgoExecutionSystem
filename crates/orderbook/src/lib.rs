//! # Order Book
//!
//! High-performance order book implementation with support for:
//!
//! - **Price-Time Priority Matching**: Standard FIFO order matching at each price level
//! - **Multiple Order Types**: Market, Limit, FOK (Fill-or-Kill), IOC (Immediate-or-Cancel)
//! - **Advanced Order Types**: Stop orders, iceberg orders, and pegged orders
//! - **Market Making**: Built-in market maker for liquidity simulation
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

pub mod types;
pub mod book;
pub mod advanced_orders;
pub mod simulation;

pub use types::*;
pub use book::*;
pub use advanced_orders::*;
pub use simulation::*;
