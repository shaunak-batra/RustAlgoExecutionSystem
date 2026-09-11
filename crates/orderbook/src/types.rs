use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Fixed-point price in integer ticks.
///
/// One tick is `1 / TICK_SCALE` currency units (five decimal places). Prices stay
/// integers inside the book, so level lookup and comparisons are exact; floating
/// point is only used at the boundaries (user input and display).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Price(pub i64);

/// Error converting a floating-point price to ticks.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum PriceError {
    #[error("price must be a finite number, got {0}")]
    NotFinite(f64),
    #[error("price {0} does not fit in the tick range")]
    OutOfRange(f64),
}

impl Price {
    pub const ZERO: Self = Price(0);
    /// Ticks per currency unit.
    pub const TICK_SCALE: f64 = 100_000.0;

    #[inline]
    pub const fn new(ticks: i64) -> Self {
        Price(ticks)
    }

    /// Converts a decimal price to the nearest tick.
    ///
    /// Rounds instead of truncating: `0.29 * 100_000.0` is `28999.999...` in
    /// floating point, which truncation would turn into the wrong tick. NaN maps
    /// to zero and out-of-range values saturate, so use [`Price::try_from_f64`]
    /// for untrusted input.
    #[inline]
    pub fn from_f64(price: f64) -> Self {
        Price((price * Self::TICK_SCALE).round() as i64)
    }

    /// Converts a decimal price to the nearest tick, rejecting NaN, infinities,
    /// and values outside the `i64` tick range.
    pub fn try_from_f64(price: f64) -> Result<Self, PriceError> {
        if !price.is_finite() {
            return Err(PriceError::NotFinite(price));
        }
        let ticks = (price * Self::TICK_SCALE).round();
        // -2^63 is exactly representable; 2^63 is one past i64::MAX.
        if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&ticks) {
            return Err(PriceError::OutOfRange(price));
        }
        Ok(Price(ticks as i64))
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
        write!(f, "{:.5}", self.as_f64())
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
    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Quantity)
    }

    #[inline]
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Quantity)
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
    /// Good-till-cancelled: any unfilled remainder rests in the book.
    GTC,
    /// Immediate-or-cancel: trade what is available now, cancel the rest.
    IOC,
    /// Fill-or-kill: trade the full quantity now, or nothing at all.
    FOK,
}

/// Unique order identifier.
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

/// Timestamp in nanoseconds since the Unix epoch (or simulation start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    #[inline]
    pub const fn new(nanos: u64) -> Self {
        Timestamp(nanos)
    }

    /// Current wall-clock time. A clock set before 1970 yields 0; values past
    /// `u64::MAX` nanoseconds (year 2554) saturate.
    #[inline]
    pub fn now_nanos() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Timestamp(nanos)
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
    fn price_rounds_to_nearest_tick() {
        // 0.29 * 100_000.0 == 28999.999999999996; truncation would give 28_999.
        assert_eq!(Price::from_f64(0.29).ticks(), 29_000);
        assert_eq!(Price::from_f64(-0.29).ticks(), -29_000);
        assert_eq!(Price::from_f64(1.1).ticks(), 110_000);
    }

    #[test]
    fn every_cent_price_converts_exactly() {
        for cents in 1..=1_000_000i64 {
            let price = cents as f64 / 100.0;
            assert_eq!(
                Price::from_f64(price).ticks(),
                cents * 1_000,
                "price {price}"
            );
        }
    }

    #[test]
    fn every_tick_price_round_trips() {
        for ticks in 1..=200_000i64 {
            let price = Price::new(ticks);
            assert_eq!(Price::from_f64(price.as_f64()), price);
        }
    }

    #[test]
    fn try_from_f64_rejects_invalid_input() {
        assert!(matches!(
            Price::try_from_f64(f64::NAN),
            Err(PriceError::NotFinite(_))
        ));
        assert!(matches!(
            Price::try_from_f64(f64::NEG_INFINITY),
            Err(PriceError::NotFinite(_))
        ));
        assert!(matches!(
            Price::try_from_f64(1e300),
            Err(PriceError::OutOfRange(_))
        ));
        assert!(matches!(
            Price::try_from_f64(-1e300),
            Err(PriceError::OutOfRange(_))
        ));
        assert_eq!(Price::try_from_f64(123.45678), Ok(Price::new(12_345_678)));
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
    fn quantity_checked_ops() {
        assert_eq!(
            Quantity::new(5).checked_add(Quantity::new(6)),
            Some(Quantity::new(11))
        );
        assert_eq!(Quantity::new(u64::MAX).checked_add(Quantity::new(1)), None);
        assert_eq!(Quantity::new(5).checked_sub(Quantity::new(6)), None);
        assert!(Quantity::ZERO.is_zero());
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
