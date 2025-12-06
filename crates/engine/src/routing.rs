use orderbook::{Order, Price, Quantity, Side};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// Routing venue (exchange/orderbook)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueId(pub String);

impl VenueId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Venue information
#[derive(Debug, Clone)]
pub struct Venue {
    pub id: VenueId,
    pub name: String,
    pub best_bid: Option<Price>,
    pub best_ask: Option<Price>,
    pub bid_liquidity: Quantity,
    pub ask_liquidity: Quantity,
    pub latency_ms: u64,
    pub fee_bps: u64, // Fee in basis points (100 = 1%)
    pub available: bool,
}

/// Routing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingStrategy {
    /// Route to venue with best price
    BestPrice,
    /// Route to venue with most liquidity
    BestLiquidity,
    /// Route to venue with lowest latency
    LowestLatency,
    /// Route to venue with lowest fees
    LowestFee,
    /// Smart routing (considers price, liquidity, and fees)
    Smart,
}

/// Routing decision
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub venue: VenueId,
    pub order: Order,
    pub reason: String,
    pub expected_price: Price,
}

/// Routing result
#[derive(Debug, Clone)]
pub enum RoutingResult {
    /// Successfully routed to venue
    Routed(RoutingDecision),
    /// No suitable venue found
    NoVenue(String),
    /// All venues failed after retries
    AllVenuesFailed(Vec<String>),
}

/// Smart Order Router
pub struct SmartRouter {
    venues: HashMap<VenueId, Venue>,
    strategy: RoutingStrategy,
    max_retries: u32,
    retry_delay_ms: u64,
}

impl SmartRouter {
    pub fn new(strategy: RoutingStrategy, max_retries: u32) -> Self {
        Self {
            venues: HashMap::new(),
            strategy,
            max_retries,
            retry_delay_ms: 100,
        }
    }

    /// Register a venue
    pub fn add_venue(&mut self, venue: Venue) {
        info!("Registering venue: {} ({})", venue.name, venue.id.0);
        self.venues.insert(venue.id.clone(), venue);
    }

    /// Update venue market data
    pub fn update_venue(
        &mut self,
        venue_id: &VenueId,
        best_bid: Option<Price>,
        best_ask: Option<Price>,
        bid_liquidity: Quantity,
        ask_liquidity: Quantity,
    ) {
        if let Some(venue) = self.venues.get_mut(venue_id) {
            venue.best_bid = best_bid;
            venue.best_ask = best_ask;
            venue.bid_liquidity = bid_liquidity;
            venue.ask_liquidity = ask_liquidity;
        }
    }

    /// Mark venue as available/unavailable
    pub fn set_venue_availability(&mut self, venue_id: &VenueId, available: bool) {
        if let Some(venue) = self.venues.get_mut(venue_id) {
            venue.available = available;
            if available {
                info!("Venue {} is now available", venue.name);
            } else {
                warn!("Venue {} is now unavailable", venue.name);
            }
        }
    }

    /// Route an order to the best venue
    pub fn route_order(&self, order: &Order) -> RoutingResult {
        let available_venues: Vec<&Venue> = self
            .venues
            .values()
            .filter(|v| v.available)
            .collect();

        if available_venues.is_empty() {
            return RoutingResult::NoVenue("No available venues".to_string());
        }

        // Select best venue based on strategy
        let best_venue = self.select_best_venue(&available_venues, order);

        match best_venue {
            Some(venue) => {
                let expected_price = match order.side {
                    Side::Buy => venue.best_ask.unwrap_or(order.price),
                    Side::Sell => venue.best_bid.unwrap_or(order.price),
                };

                RoutingResult::Routed(RoutingDecision {
                    venue: venue.id.clone(),
                    order: order.clone(),
                    reason: self.get_routing_reason(venue, order),
                    expected_price,
                })
            }
            None => RoutingResult::NoVenue("No suitable venue found".to_string()),
        }
    }

    /// Select best venue based on strategy
    fn select_best_venue<'a>(
        &self,
        venues: &[&'a Venue],
        order: &Order,
    ) -> Option<&'a Venue> {
        match self.strategy {
            RoutingStrategy::BestPrice => self.select_by_best_price(venues, order),
            RoutingStrategy::BestLiquidity => self.select_by_liquidity(venues, order),
            RoutingStrategy::LowestLatency => self.select_by_latency(venues),
            RoutingStrategy::LowestFee => self.select_by_fees(venues),
            RoutingStrategy::Smart => self.select_smart(venues, order),
        }
    }

    fn select_by_best_price<'a>(
        &self,
        venues: &[&'a Venue],
        order: &Order,
    ) -> Option<&'a Venue> {
        let mut best_venue: Option<&Venue> = None;
        let mut best_price: Option<Price> = None;

        for venue in venues {
            let venue_price = match order.side {
                Side::Buy => venue.best_ask,
                Side::Sell => venue.best_bid,
            };

            if let Some(price) = venue_price {
                match best_price {
                    None => {
                        best_price = Some(price);
                        best_venue = Some(venue);
                    }
                    Some(bp) => {
                        let is_better = match order.side {
                            Side::Buy => price < bp,  // Lower ask is better for buy
                            Side::Sell => price > bp, // Higher bid is better for sell
                        };
                        if is_better {
                            best_price = Some(price);
                            best_venue = Some(venue);
                        }
                    }
                }
            }
        }

        best_venue
    }

    fn select_by_liquidity<'a>(
        &self,
        venues: &[&'a Venue],
        order: &Order,
    ) -> Option<&'a Venue> {
        venues
            .iter()
            .max_by_key(|v| match order.side {
                Side::Buy => v.ask_liquidity,
                Side::Sell => v.bid_liquidity,
            })
            .copied()
    }

    fn select_by_latency<'a>(&self, venues: &[&'a Venue]) -> Option<&'a Venue> {
        venues.iter().min_by_key(|v| v.latency_ms).copied()
    }

    fn select_by_fees<'a>(&self, venues: &[&'a Venue]) -> Option<&'a Venue> {
        venues.iter().min_by_key(|v| v.fee_bps).copied()
    }

    fn select_smart<'a>(&self, venues: &[&'a Venue], order: &Order) -> Option<&'a Venue> {
        // Smart routing: score = price_score + liquidity_score - latency_penalty - fee_penalty
        let mut best_venue: Option<&Venue> = None;
        let mut best_score = f64::MIN;

        for venue in venues {
            let venue_price = match order.side {
                Side::Buy => venue.best_ask,
                Side::Sell => venue.best_bid,
            };

            if let Some(price) = venue_price {
                // Calculate composite score
                let price_score = match order.side {
                    Side::Buy => 1.0 / (price.as_f64() + 1.0), // Lower price = higher score
                    Side::Sell => price.as_f64(),                // Higher price = higher score
                };

                let liquidity = match order.side {
                    Side::Buy => venue.ask_liquidity.value() as f64,
                    Side::Sell => venue.bid_liquidity.value() as f64,
                };
                // Protect against ln(0) = -infinity and ensure no NaN
                let liquidity_score = if liquidity > 0.0 {
                    liquidity.ln().max(0.0)
                } else {
                    0.0
                };

                let latency_penalty = venue.latency_ms as f64 * 0.01;
                let fee_penalty = venue.fee_bps as f64 * 0.1;

                let score = price_score * 100.0 + liquidity_score - latency_penalty - fee_penalty;

                if score > best_score {
                    best_score = score;
                    best_venue = Some(venue);
                }
            }
        }

        best_venue
    }

    fn get_routing_reason(&self, venue: &Venue, order: &Order) -> String {
        match self.strategy {
            RoutingStrategy::BestPrice => {
                let price = match order.side {
                    Side::Buy => venue.best_ask.unwrap_or(Price::ZERO),
                    Side::Sell => venue.best_bid.unwrap_or(Price::ZERO),
                };
                format!("Best price: {}", price)
            }
            RoutingStrategy::BestLiquidity => {
                let liq = match order.side {
                    Side::Buy => venue.ask_liquidity,
                    Side::Sell => venue.bid_liquidity,
                };
                format!("Best liquidity: {}", liq)
            }
            RoutingStrategy::LowestLatency => {
                format!("Lowest latency: {}ms", venue.latency_ms)
            }
            RoutingStrategy::LowestFee => {
                format!("Lowest fee: {}bps", venue.fee_bps)
            }
            RoutingStrategy::Smart => {
                format!("Smart routing to {}", venue.name)
            }
        }
    }

    /// Route with automatic retry on failure
    /// This is thread-safe as it doesn't mutate router state
    pub fn route_with_retry(&self, order: &Order) -> RoutingResult {
        let mut failed_venues = Vec::new();
        let mut attempts = 0;

        while attempts < self.max_retries {
            match self.route_order(order) {
                RoutingResult::Routed(decision) => {
                    info!(
                        "Order {} routed to {} (attempt {}): {}",
                        order.id,
                        decision.venue.0,
                        attempts + 1,
                        decision.reason
                    );
                    return RoutingResult::Routed(decision);
                }
                RoutingResult::NoVenue(reason) => {
                    warn!("Routing attempt {} failed: {}", attempts + 1, reason);
                    failed_venues.push(reason);
                    // Note: We don't mutate venue availability here for thread safety
                    // Venue availability should be managed by the caller based on actual connectivity
                }
                RoutingResult::AllVenuesFailed(reasons) => {
                    failed_venues.extend(reasons);
                    break;
                }
            }

            attempts += 1;
            std::thread::sleep(std::time::Duration::from_millis(self.retry_delay_ms));
        }

        warn!(
            "Order {} routing failed after {} attempts",
            order.id, attempts
        );
        RoutingResult::AllVenuesFailed(failed_venues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orderbook::{OrderId, Timestamp};

    fn create_test_venue(
        id: &str,
        best_bid: f64,
        best_ask: f64,
        liquidity: u64,
        latency_ms: u64,
        fee_bps: u64,
    ) -> Venue {
        Venue {
            id: VenueId::new(id),
            name: id.to_string(),
            best_bid: Some(Price::from_f64(best_bid)),
            best_ask: Some(Price::from_f64(best_ask)),
            bid_liquidity: Quantity::new(liquidity),
            ask_liquidity: Quantity::new(liquidity),
            latency_ms,
            fee_bps,
            available: true,
        }
    }

    fn create_test_order(side: Side, price: f64, qty: u64) -> Order {
        Order {
            id: OrderId::new(1),
            side,
            price: Price::from_f64(price),
            qty: Quantity::new(qty),
            timestamp: Timestamp::new(1000),
            tif: orderbook::TimeInForce::GTC,
        }
    }

    #[test]
    fn test_best_price_routing() {
        let mut router = SmartRouter::new(RoutingStrategy::BestPrice, 3);

        router.add_venue(create_test_venue("Exchange_A", 50000.0, 50010.0, 100, 10, 10));
        router.add_venue(create_test_venue("Exchange_B", 50005.0, 50008.0, 50, 20, 15));

        let buy_order = create_test_order(Side::Buy, 50010.0, 10);
        let result = router.route_order(&buy_order);

        match result {
            RoutingResult::Routed(decision) => {
                assert_eq!(decision.venue.0, "Exchange_B"); // Better ask price
            }
            _ => panic!("Expected successful routing"),
        }
    }

    #[test]
    fn test_best_liquidity_routing() {
        let mut router = SmartRouter::new(RoutingStrategy::BestLiquidity, 3);

        router.add_venue(create_test_venue("Exchange_A", 50000.0, 50010.0, 100, 10, 10));
        router.add_venue(create_test_venue("Exchange_B", 50005.0, 50008.0, 50, 20, 15));

        let buy_order = create_test_order(Side::Buy, 50010.0, 10);
        let result = router.route_order(&buy_order);

        match result {
            RoutingResult::Routed(decision) => {
                assert_eq!(decision.venue.0, "Exchange_A"); // More liquidity
            }
            _ => panic!("Expected successful routing"),
        }
    }

    #[test]
    fn test_no_venue_available() {
        let mut router = SmartRouter::new(RoutingStrategy::BestPrice, 3);

        router.add_venue(create_test_venue("Exchange_A", 50000.0, 50010.0, 100, 10, 10));
        router.set_venue_availability(&VenueId::new("Exchange_A"), false);

        let buy_order = create_test_order(Side::Buy, 50010.0, 10);
        let result = router.route_order(&buy_order);

        assert!(matches!(result, RoutingResult::NoVenue(_)));
    }

    #[test]
    fn test_smart_routing() {
        let mut router = SmartRouter::new(RoutingStrategy::Smart, 3);

        // Exchange A: Best price but high fees
        router.add_venue(create_test_venue("Exchange_A", 50000.0, 50005.0, 100, 10, 50));
        // Exchange B: Moderate price, low fees, good liquidity
        router.add_venue(create_test_venue("Exchange_B", 50000.0, 50008.0, 200, 15, 5));
        // Exchange C: Worst price
        router.add_venue(create_test_venue("Exchange_C", 50000.0, 50015.0, 150, 5, 10));

        let buy_order = create_test_order(Side::Buy, 50010.0, 10);
        let result = router.route_order(&buy_order);

        match result {
            RoutingResult::Routed(decision) => {
                // Smart routing should prefer Exchange_B (balanced price, liquidity, fees)
                assert_eq!(decision.venue.0, "Exchange_B");
            }
            _ => panic!("Expected successful routing"),
        }
    }
}
