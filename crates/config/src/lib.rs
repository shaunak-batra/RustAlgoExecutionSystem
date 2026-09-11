//! Engine server settings, loaded from TOML and validated before use.
//!
//! Unknown keys are rejected, so a misspelled setting fails loudly instead of
//! silently falling back to a default. Money amounts are in price units here;
//! the engine converts them to integer ticks.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use thiserror::Error;

/// All engine server settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub engine: EngineSettings,
    pub api: ApiSettings,
    pub risk: RiskSettings,
    pub venue: VenueSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSettings {
    /// How often due child orders are released and simulated quotes are
    /// refreshed, in milliseconds (1 to 60 000).
    pub tick_interval_ms: u64,
    /// Capacity of the command channel from the API to the engine task.
    pub command_buffer: usize,
    /// How many fills a slow stream subscriber may fall behind before its
    /// stream ends with DATA_LOSS.
    pub fill_buffer: usize,
    /// Finished parent orders kept for status queries.
    pub retained_finished_orders: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSettings {
    /// Address the gRPC server listens on. The API has no authentication, so
    /// keep it on a loopback address.
    pub listen_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskSettings {
    /// Largest quantity of a single parent order.
    pub max_order_qty: u64,
    /// Largest notional of a single parent order (price units × units).
    pub max_order_notional: f64,
    /// Largest absolute position per symbol, counting working orders.
    pub max_position_qty: u64,
    /// Largest gross exposure across symbols (price units × units).
    pub max_gross_notional: f64,
    /// Largest distance of a limit price from the mid, in basis points.
    pub price_collar_bps: u32,
    /// Trading halts once total P&L falls to `-max_loss` (price units × units).
    pub max_loss: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueSettings {
    pub symbols: Vec<SymbolSettings>,
}

/// One simulated market.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolSettings {
    pub symbol: String,
    /// Mid price the simulated market maker quotes around.
    pub reference_price: f64,
    /// Price levels quoted on each side.
    pub levels: u32,
    pub qty_per_level: u64,
    /// Gap between levels in basis points of the reference price.
    pub level_spacing_bps: u32,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid settings file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid setting {field}: {reason}")]
    Invalid { field: String, reason: String },
}

impl Settings {
    /// Reads, parses and validates a settings file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SettingsError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml_str(&text)
    }

    /// Parses and validates settings from TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self, SettingsError> {
        let settings: Settings = toml::from_str(text)?;
        settings.validate()?;
        Ok(settings)
    }

    /// Checks every value range; the error names the offending field.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let engine = &self.engine;
        check(
            "engine.tick_interval_ms",
            (1..=60_000).contains(&engine.tick_interval_ms),
            "must be between 1 and 60000",
        )?;
        check(
            "engine.command_buffer",
            engine.command_buffer > 0,
            "must be positive",
        )?;
        check(
            "engine.fill_buffer",
            engine.fill_buffer > 0,
            "must be positive",
        )?;
        check(
            "engine.retained_finished_orders",
            engine.retained_finished_orders > 0,
            "must be positive",
        )?;

        let risk = &self.risk;
        check(
            "risk.max_order_qty",
            risk.max_order_qty > 0,
            "must be positive",
        )?;
        check(
            "risk.max_position_qty",
            risk.max_position_qty > 0,
            "must be positive",
        )?;
        positive_amount("risk.max_order_notional", risk.max_order_notional)?;
        positive_amount("risk.max_gross_notional", risk.max_gross_notional)?;
        positive_amount("risk.max_loss", risk.max_loss)?;
        check(
            "risk.price_collar_bps",
            (1..=10_000).contains(&risk.price_collar_bps),
            "must be between 1 and 10000",
        )?;

        check(
            "venue.symbols",
            !self.venue.symbols.is_empty(),
            "must list at least one symbol",
        )?;
        let mut seen = BTreeSet::new();
        for (index, symbol) in self.venue.symbols.iter().enumerate() {
            let field = |name: &str| format!("venue.symbols[{index}].{name}");
            check(
                &field("symbol"),
                !symbol.symbol.trim().is_empty(),
                "must not be empty",
            )?;
            check(
                &field("symbol"),
                seen.insert(symbol.symbol.as_str()),
                "is listed more than once",
            )?;
            positive_amount(&field("reference_price"), symbol.reference_price)?;
            check(
                &field("levels"),
                (1..=1_000).contains(&symbol.levels),
                "must be between 1 and 1000",
            )?;
            check(
                &field("qty_per_level"),
                symbol.qty_per_level > 0,
                "must be positive",
            )?;
            check(
                &field("level_spacing_bps"),
                symbol.level_spacing_bps > 0,
                "must be positive",
            )?;
            check(
                &field("level_spacing_bps"),
                u64::from(symbol.levels) * u64::from(symbol.level_spacing_bps) < 10_000,
                "levels x level_spacing_bps must be below 10000 so every bid stays above zero",
            )?;
        }
        Ok(())
    }
}

fn check(field: &str, ok: bool, reason: &str) -> Result<(), SettingsError> {
    if ok {
        Ok(())
    } else {
        Err(SettingsError::Invalid {
            field: field.to_string(),
            reason: reason.to_string(),
        })
    }
}

fn positive_amount(field: &str, value: f64) -> Result<(), SettingsError> {
    check(
        field,
        value.is_finite() && value > 0.0,
        "must be a finite positive number",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = include_str!("../../../config/default.toml");

    fn invalid_field(settings: &Settings) -> String {
        match settings.validate() {
            Err(SettingsError::Invalid { field, .. }) => field,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn shipped_default_config_is_valid() {
        let settings = Settings::from_toml_str(SHIPPED).unwrap();
        assert!(settings.api.listen_addr.ip().is_loopback());
        assert!(!settings.venue.symbols.is_empty());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let misspelled = SHIPPED.replace("[engine]", "[engine]\ntick_intervall_ms = 5");
        assert!(matches!(
            Settings::from_toml_str(&misspelled),
            Err(SettingsError::Parse(_))
        ));
    }

    #[test]
    fn invalid_values_name_the_field() {
        let base = Settings::from_toml_str(SHIPPED).unwrap();

        let mut settings = base.clone();
        settings.engine.tick_interval_ms = 0;
        assert_eq!(invalid_field(&settings), "engine.tick_interval_ms");

        let mut settings = base.clone();
        settings.risk.max_loss = f64::NAN;
        assert_eq!(invalid_field(&settings), "risk.max_loss");

        let mut settings = base.clone();
        settings.risk.price_collar_bps = 10_001;
        assert_eq!(invalid_field(&settings), "risk.price_collar_bps");

        let mut settings = base.clone();
        settings.venue.symbols.clear();
        assert_eq!(invalid_field(&settings), "venue.symbols");

        let mut settings = base.clone();
        let duplicate = settings.venue.symbols[0].clone();
        settings.venue.symbols.push(duplicate);
        assert_eq!(
            invalid_field(&settings),
            format!("venue.symbols[{}].symbol", base.venue.symbols.len())
        );

        let mut settings = base.clone();
        settings.venue.symbols[0].reference_price = -1.0;
        assert_eq!(invalid_field(&settings), "venue.symbols[0].reference_price");

        let mut settings = base;
        settings.venue.symbols[0].levels = 100;
        settings.venue.symbols[0].level_spacing_bps = 100;
        assert_eq!(
            invalid_field(&settings),
            "venue.symbols[0].level_spacing_bps"
        );
    }

    #[test]
    fn missing_file_is_a_read_error() {
        assert!(matches!(
            Settings::load("does/not/exist.toml"),
            Err(SettingsError::Read { .. })
        ));
    }
}
