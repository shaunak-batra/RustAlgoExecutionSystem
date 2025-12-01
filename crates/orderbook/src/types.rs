use serde::{Deserialize, Serialize};
use std::fmt;

/// Newtype for prices (in basis points or ticks) to prevent accidental arithmetic with quantities.
/// Using i64 to allow for signed prices if needed (e.g., spreads).
/// Example: 100_000 = $1.00 if tick=0.0001
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Price(pub i64);

impl Price {
    pub const ZERO: Self = Price(0);
    pub const TICK_SCALE: f64 = 100_000.0;

    #[inline]
    pub const fn new(ticks: i64) -> Self {
        Price(ticks)
    }

    #[inline]
    pub fn from_f64(price: f64) -> Self {
        Price((price * Self::TICK_SCALE) as i64)
    }

    #[inline]
    pub fn as_f64(&self) -> f64 {
        self.0 as f64 / Self::TICK_SCALE
    }

    #[inline]
    pub const fn ticks(&self) -> i64 {
        self.0
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${:.5}", self.as_f64())
    }
}

/// Newtype for quantities (integer lots or shares).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Quantity(pub u64);

impl Quantity {
    pub const ZERO: Self = Quantity(0);

    #[inline]
    pub const fn new(qty: u64) -> Self {
        Quantity(qty)
    }

    #[inline]
    pub const fn value(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn saturating_sub(self, other: Self) -> Self {
        Quantity(self.0.saturating_sub(other.0))
    }

    #[inline]
    pub fn saturating_add(self, other: Self) -> Self {
        Quantity(self.0.saturating_add(other.0))
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Order side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(&self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

impl std::str::FromStr for Side {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "BUY" | "B" => Ok(Side::Buy),
            "SELL" | "S" => Ok(Side::Sell),
            _ => Err(format!("Invalid side: {}", s)),
        }
    }
}

/// Time-in-force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    /// Good-till-cancelled
    GTC,
    /// Immediate-or-cancel
    IOC,
    /// Fill-or-kill
    FOK,
}

/// Unique order identifier (u64 for simplicity; consider UUIDv7 for distributed systems).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct OrderId(pub u64);

impl OrderId {
    #[inline]
    pub const fn new(id: u64) -> Self {
        OrderId(id)
    }

    #[inline]
    pub const fn value(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Order#{}", self.0)
    }
}

/// Timestamp in nanoseconds since epoch (or simulation start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    #[inline]
    pub const fn new(nanos: u64) -> Self {
        Timestamp(nanos)
    }

    #[inline]
    pub fn now_nanos() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_nanos() as u64,
        )
    }

    #[inline]
    pub const fn nanos(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn as_secs_f64(&self) -> f64 {
        self.0 as f64 / 1_000_000_000.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ns", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_conversions() {
        let price = Price::from_f64(42.50);
        assert_eq!(price.ticks(), 4_250_000);
        assert!((price.as_f64() - 42.50).abs() < 0.00001);
    }

    #[test]
    fn test_price_ordering() {
        let p1 = Price::new(100);
        let p2 = Price::new(200);
        assert!(p1 < p2);
    }

    #[test]
    fn test_quantity_saturating_ops() {
        let q1 = Quantity::new(100);
        let q2 = Quantity::new(50);
        assert_eq!(q1.saturating_sub(q2), Quantity::new(50));
        assert_eq!(q2.saturating_sub(q1), Quantity::ZERO);
    }

    #[test]
    fn test_side_parsing() {
        use std::str::FromStr;
        assert_eq!(Side::from_str("BUY").unwrap(), Side::Buy);
        assert_eq!(Side::from_str("buy").unwrap(), Side::Buy);
        assert_eq!(Side::from_str("SELL").unwrap(), Side::Sell);
        assert_eq!(Side::from_str("B").unwrap(), Side::Buy);
        assert!(Side::from_str("INVALID").is_err());
    }

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn test_timestamp_creation() {
        let ts1 = Timestamp::now_nanos();
        let ts2 = Timestamp::now_nanos();
        assert!(ts2.nanos() >= ts1.nanos());
    }
}
