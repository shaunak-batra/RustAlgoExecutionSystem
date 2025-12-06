use database::{Database, DatabaseResult, MarketTick};
use orderbook::{Price, Timestamp};
use std::collections::VecDeque;
use tracing::info;

/// Historical market data event
#[derive(Debug, Clone)]
pub struct ReplayEvent {
    pub timestamp_ns: u64,
    pub symbol: String,
    pub bid_price: Price,
    pub ask_price: Price,
    pub mid_price: Price,
    pub spread: f64,
    pub bid_qty: u64,
    pub ask_qty: u64,
    pub last_price: Price,
    pub volume_24h: f64,
}

impl From<MarketTick> for ReplayEvent {
    fn from(tick: MarketTick) -> Self {
        Self {
            timestamp_ns: tick.timestamp_ns,
            symbol: tick.symbol,
            bid_price: Price::from_f64(tick.bid_price),
            ask_price: Price::from_f64(tick.ask_price),
            mid_price: Price::from_f64(tick.mid_price),
            spread: tick.spread,
            bid_qty: tick.bid_qty,
            ask_qty: tick.ask_qty,
            last_price: Price::from_f64(tick.last_price),
            volume_24h: tick.volume_24h,
        }
    }
}

/// Replay speed configuration
#[derive(Debug, Clone, Copy)]
pub enum ReplaySpeed {
    /// Real-time (1x speed)
    RealTime,
    /// Fast forward (10x, 100x, etc.)
    FastForward(u32),
    /// As fast as possible (no delays)
    Maximum,
}

/// Historical replay engine
pub struct HistoricalReplay {
    db: Database,
    events: VecDeque<ReplayEvent>,
    current_time: u64,
    speed: ReplaySpeed,
    paused: bool,
}

impl HistoricalReplay {
    /// Create new replay engine from database
    pub fn new(db: Database) -> Self {
        Self {
            db,
            events: VecDeque::new(),
            current_time: 0,
            speed: ReplaySpeed::RealTime,
            paused: false,
        }
    }

    /// Load historical data for a symbol and time range
    pub fn load_history(
        &mut self,
        symbol: &str,
        start_time_ns: u64,
        end_time_ns: u64,
    ) -> DatabaseResult<usize> {
        info!(
            "Loading history for {} from {} to {}",
            symbol, start_time_ns, end_time_ns
        );

        let ticks = self.db.get_market_history(symbol, Some(start_time_ns), Some(end_time_ns), None)?;

        let count = ticks.len();
        self.events.extend(ticks.into_iter().map(ReplayEvent::from));

        // Sort by timestamp
        let mut events_vec: Vec<_> = self.events.drain(..).collect();
        events_vec.sort_by_key(|e| e.timestamp_ns);
        self.events.extend(events_vec);

        if !self.events.is_empty() {
            self.current_time = self.events[0].timestamp_ns;
        }

        info!("Loaded {} historical events", count);
        Ok(count)
    }

    /// Set replay speed
    pub fn set_speed(&mut self, speed: ReplaySpeed) {
        self.speed = speed;
        info!("Replay speed set to: {:?}", speed);
    }

    /// Pause replay
    pub fn pause(&mut self) {
        self.paused = true;
        info!("Replay paused at time {}", self.current_time);
    }

    /// Resume replay
    pub fn resume(&mut self) {
        self.paused = false;
        info!("Replay resumed at time {}", self.current_time);
    }

    /// Check if replay is finished
    pub fn is_finished(&self) -> bool {
        self.events.is_empty()
    }

    /// Get current simulation time
    pub fn current_timestamp(&self) -> Timestamp {
        Timestamp::new(self.current_time)
    }

    /// Get next event without advancing
    pub fn peek_next(&self) -> Option<&ReplayEvent> {
        self.events.front()
    }

    /// Get next event and advance time
    pub fn next_event(&mut self) -> Option<ReplayEvent> {
        if self.paused {
            return None;
        }

        if let Some(event) = self.events.pop_front() {
            self.current_time = event.timestamp_ns;
            Some(event)
        } else {
            None
        }
    }

    /// Get all events up to a specific time
    pub fn drain_until(&mut self, target_time: u64) -> Vec<ReplayEvent> {
        if self.paused {
            return Vec::new();
        }

        let mut result = Vec::new();

        while let Some(event) = self.events.front() {
            if event.timestamp_ns <= target_time {
                let event = self.events.pop_front().unwrap();
                self.current_time = event.timestamp_ns;
                result.push(event);
            } else {
                break;
            }
        }

        result
    }

    /// Skip forward to a specific time
    pub fn seek_to(&mut self, target_time: u64) -> usize {
        let mut skipped = 0;

        while let Some(event) = self.events.front() {
            if event.timestamp_ns < target_time {
                self.events.pop_front();
                skipped += 1;
            } else {
                break;
            }
        }

        self.current_time = target_time;
        info!("Skipped {} events to time {}", skipped, target_time);
        skipped
    }

    /// Get statistics about loaded data
    pub fn get_stats(&self) -> ReplayStats {
        let mut symbols = std::collections::HashSet::new();
        let mut min_time = u64::MAX;
        let mut max_time = 0u64;

        for event in &self.events {
            symbols.insert(event.symbol.clone());
            min_time = min_time.min(event.timestamp_ns);
            max_time = max_time.max(event.timestamp_ns);
        }

        ReplayStats {
            total_events: self.events.len(),
            symbols: symbols.into_iter().collect(),
            start_time: if min_time != u64::MAX { Some(min_time) } else { None },
            end_time: if max_time != 0 { Some(max_time) } else { None },
            current_time: self.current_time,
        }
    }

    /// Calculate delay for real-time replay
    pub fn calculate_delay(&self, next_event: &ReplayEvent) -> Option<std::time::Duration> {
        if self.paused {
            return None;
        }

        match self.speed {
            ReplaySpeed::Maximum => None,
            ReplaySpeed::RealTime => {
                let time_diff = next_event.timestamp_ns.saturating_sub(self.current_time);
                Some(std::time::Duration::from_nanos(time_diff))
            }
            ReplaySpeed::FastForward(multiplier) => {
                let time_diff = next_event.timestamp_ns.saturating_sub(self.current_time);
                let adjusted = time_diff / multiplier as u64;
                Some(std::time::Duration::from_nanos(adjusted))
            }
        }
    }
}

/// Replay statistics
#[derive(Debug, Clone)]
pub struct ReplayStats {
    pub total_events: usize,
    pub symbols: Vec<String>,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub current_time: u64,
}

impl ReplayStats {
    pub fn duration_secs(&self) -> f64 {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => (end - start) as f64 / 1_000_000_000.0,
            _ => 0.0,
        }
    }

    pub fn progress_pct(&self) -> f64 {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) if end > start => {
                ((self.current_time - start) as f64 / (end - start) as f64) * 100.0
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_event_from_tick() {
        let tick = MarketTick {
            tick_id: 1,
            symbol: "BTCUSDT".to_string(),
            bid_price: 50000.0,
            ask_price: 50001.0,
            mid_price: 50000.5,
            spread: 1.0,
            bid_qty: 100,
            ask_qty: 150,
            last_price: 50000.5,
            volume_24h: 1000000.0,
            timestamp_ns: 1000000,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };

        let event: ReplayEvent = tick.into();
        assert_eq!(event.symbol, "BTCUSDT");
        assert_eq!(event.timestamp_ns, 1000000);
        assert_eq!(event.bid_price, Price::from_f64(50000.0));
    }

    #[test]
    fn test_replay_speed_variants() {
        let db = Database::in_memory().unwrap();
        let mut replay = HistoricalReplay::new(db);

        replay.set_speed(ReplaySpeed::RealTime);
        replay.set_speed(ReplaySpeed::FastForward(10));
        replay.set_speed(ReplaySpeed::Maximum);
    }

    #[test]
    fn test_replay_pause_resume() {
        let db = Database::in_memory().unwrap();
        let mut replay = HistoricalReplay::new(db);

        assert!(!replay.paused);

        replay.pause();
        assert!(replay.paused);

        replay.resume();
        assert!(!replay.paused);
    }

    #[test]
    fn test_replay_stats() {
        let db = Database::in_memory().unwrap();
        let replay = HistoricalReplay::new(db);

        let stats = replay.get_stats();
        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.progress_pct(), 0.0);
    }
}
