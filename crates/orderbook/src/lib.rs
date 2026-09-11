//! # Order Book
//!
//! Single-symbol limit order book with price-time priority matching.
//!
//! - Trades execute at the resting (maker) order's price: best price first,
//!   then arrival order within a price level.
//! - Time in force: `GTC` rests any remainder, `IOC` cancels it, and `FOK`
//!   trades the full quantity or nothing.
//! - Market orders (no limit price) must be `IOC` or `FOK`.
//! - Invalid orders are rejected with an [`OrderError`] and leave the book
//!   unchanged.
//!
//! ## Key Components
//!
//! - [`OrderBook`]: price levels, order-id index, and matching
//! - [`Order`], [`Fill`], [`SubmitResult`], [`OrderOutcome`]: inputs and execution reports
//! - [`Price`], [`Quantity`], [`OrderId`], [`Timestamp`]: integer newtypes
//!
//! ## Example
//!
//! ```
//! use orderbook::{Order, OrderBook, OrderId, OrderOutcome, Price, Quantity, Side, Timestamp};
//!
//! let mut book = OrderBook::new("BTC-USD".to_string());
//!
//! // Resting ask: sell 10 @ 100.00
//! let ask = Order::limit(OrderId(1), Side::Sell, Price::from_f64(100.0), Quantity(10), Timestamp(1));
//! book.submit_order(ask).unwrap();
//!
//! // A buy limit at 101.00 for 4 trades against it at the maker's price.
//! let bid = Order::limit(OrderId(2), Side::Buy, Price::from_f64(101.0), Quantity(4), Timestamp(2));
//! let result = book.submit_order(bid).unwrap();
//!
//! assert_eq!(result.outcome, OrderOutcome::Filled);
//! assert_eq!(result.fills[0].price, Price::from_f64(100.0));
//! assert_eq!(book.ask_qty_at(Price::from_f64(100.0)), Quantity(6));
//! ```

pub mod book;
pub mod types;

pub use book::*;
pub use types::*;
