use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub engine: EngineConfig,
    pub market_data: MarketDataConfig,
    pub risk: RiskConfig,
    pub database: DatabaseConfig,
    pub api: ApiConfig,
    pub gui: GuiConfig,
    pub algorithms: AlgorithmConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Clock tick interval in milliseconds
    pub clock_tick_interval_ms: u64,
    /// Maximum pending orders in the system
    pub max_pending_orders: usize,
    /// Enable order routing
    pub enable_smart_routing: bool,
    /// Routing retry attempts
    pub routing_retry_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataConfig {
    /// Market data mode: "websocket" or "rest_api"
    pub mode: String,
    /// WebSocket URL (e.g., wss://stream.binance.com:9443/ws)
    pub websocket_url: String,
    /// REST API URL (e.g., https://api.binance.com)
    pub rest_api_url: String,
    /// Fallback to REST API if WebSocket fails
    pub fallback_on_ws_failure: bool,
    /// WebSocket reconnection attempts before fallback
    pub ws_reconnect_attempts: u32,
    /// Tick sampling interval in milliseconds (e.g., 10000 for 10 seconds)
    pub tick_sample_interval_ms: u64,
    /// REST API polling interval in milliseconds
    pub rest_poll_interval_ms: u64,
    /// Symbols to subscribe to
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    /// Maximum position size per symbol
    pub max_position_size: f64,
    /// Maximum total exposure across all symbols
    pub max_total_exposure: f64,
    /// Stop loss percentage (e.g., 0.02 for 2%)
    pub stop_loss_pct: f64,
    /// Take profit percentage (e.g., 0.05 for 5%)
    pub take_profit_pct: f64,
    /// Enable real-time risk checks
    pub enable_risk_checks: bool,
    /// Maximum daily loss limit
    pub max_daily_loss: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database file path
    pub path: String,
    /// Enable database persistence
    pub enable_persistence: bool,
    /// Enable tick data recording
    pub enable_tick_recording: bool,
    /// Auto-vacuum database periodically
    pub auto_vacuum: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// gRPC server host
    pub host: String,
    /// gRPC server port
    pub port: u16,
    /// Enable server reflection (for grpcurl/debugging)
    pub enable_reflection: bool,
    /// Max concurrent streams
    pub max_concurrent_streams: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiConfig {
    /// Window title
    pub window_title: String,
    /// Initial window width
    pub window_width: u32,
    /// Initial window height
    pub window_height: u32,
    /// Theme: "light" or "dark"
    pub theme: String,
    /// Chart update interval in milliseconds
    pub chart_update_interval_ms: u64,
    /// News update interval in milliseconds
    pub news_update_interval_ms: u64,
    /// Enable keyboard shortcuts
    pub enable_keyboard_shortcuts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgorithmConfig {
    pub twap: TwapConfig,
    pub pov: PovConfig,
    pub vwap: VwapConfig,
    pub is_algo: IsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwapConfig {
    /// Default slice duration in seconds
    pub default_slice_duration_secs: u64,
    /// Enable adaptive slicing
    pub enable_adaptive: bool,
    /// Adaptive volatility window (number of ticks)
    pub adaptive_volatility_window: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PovConfig {
    /// Default participation rate (0.0 to 1.0)
    pub default_participation_rate: f64,
    /// Minimum participation rate
    pub min_participation_rate: f64,
    /// Maximum participation rate
    pub max_participation_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VwapConfig {
    /// Volume profile calculation window in seconds
    pub volume_profile_window_secs: u64,
    /// Enable intraday volume curve
    pub enable_intraday_curve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsConfig {
    /// Risk aversion parameter (lambda in Almgren-Chriss)
    pub risk_aversion: f64,
    /// Market impact model: "almgren_chriss" or "linear"
    pub impact_model: String,
}

impl SystemConfig {
    /// Load configuration from a TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;

        let config: SystemConfig = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.as_ref().display()))?;

        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a string
    pub fn from_str(contents: &str) -> Result<Self> {
        let config: SystemConfig = toml::from_str(contents)
            .context("Failed to parse config from string")?;

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        // Validate market data config
        if self.market_data.mode != "websocket" && self.market_data.mode != "rest_api" {
            anyhow::bail!("market_data.mode must be either 'websocket' or 'rest_api'");
        }

        if self.market_data.tick_sample_interval_ms == 0 {
            anyhow::bail!("market_data.tick_sample_interval_ms must be greater than 0");
        }

        if self.market_data.symbols.is_empty() {
            anyhow::bail!("market_data.symbols cannot be empty");
        }

        // Validate risk config
        if self.risk.max_position_size <= 0.0 {
            anyhow::bail!("risk.max_position_size must be positive");
        }

        if self.risk.max_total_exposure <= 0.0 {
            anyhow::bail!("risk.max_total_exposure must be positive");
        }

        if self.risk.stop_loss_pct < 0.0 || self.risk.stop_loss_pct > 1.0 {
            anyhow::bail!("risk.stop_loss_pct must be between 0.0 and 1.0");
        }

        if self.risk.take_profit_pct <= 0.0 {
            anyhow::bail!("risk.take_profit_pct must be positive");
        }

        // Validate GUI config
        if self.gui.theme != "light" && self.gui.theme != "dark" {
            anyhow::bail!("gui.theme must be either 'light' or 'dark'");
        }

        // Validate algorithm configs
        if self.algorithms.pov.default_participation_rate < self.algorithms.pov.min_participation_rate
            || self.algorithms.pov.default_participation_rate > self.algorithms.pov.max_participation_rate
        {
            anyhow::bail!(
                "pov.default_participation_rate must be between min_participation_rate and max_participation_rate"
            );
        }

        if self.algorithms.is_algo.risk_aversion < 0.0 {
            anyhow::bail!("is_algo.risk_aversion must be non-negative");
        }

        Ok(())
    }

    /// Create a default configuration
    pub fn default() -> Self {
        Self {
            engine: EngineConfig {
                clock_tick_interval_ms: 100,
                max_pending_orders: 10000,
                enable_smart_routing: false,
                routing_retry_attempts: 3,
            },
            market_data: MarketDataConfig {
                mode: "rest_api".to_string(),
                websocket_url: "wss://stream.binance.com:9443/ws".to_string(),
                rest_api_url: "https://api.binance.com".to_string(),
                fallback_on_ws_failure: true,
                ws_reconnect_attempts: 5,
                tick_sample_interval_ms: 10000,
                rest_poll_interval_ms: 2000,
                symbols: vec!["btcusdt".to_string(), "ethusdt".to_string()],
            },
            risk: RiskConfig {
                max_position_size: 100000.0,
                max_total_exposure: 500000.0,
                stop_loss_pct: 0.02,
                take_profit_pct: 0.05,
                enable_risk_checks: true,
                max_daily_loss: 10000.0,
            },
            database: DatabaseConfig {
                path: "trading_system.db".to_string(),
                enable_persistence: true,
                enable_tick_recording: true,
                auto_vacuum: true,
            },
            api: ApiConfig {
                host: "127.0.0.1".to_string(),
                port: 50051,
                enable_reflection: true,
                max_concurrent_streams: 100,
            },
            gui: GuiConfig {
                window_title: "Algorithmic Trading System".to_string(),
                window_width: 1400,
                window_height: 900,
                theme: "dark".to_string(),
                chart_update_interval_ms: 500,
                news_update_interval_ms: 5000,
                enable_keyboard_shortcuts: true,
            },
            algorithms: AlgorithmConfig {
                twap: TwapConfig {
                    default_slice_duration_secs: 60,
                    enable_adaptive: false,
                    adaptive_volatility_window: 20,
                },
                pov: PovConfig {
                    default_participation_rate: 0.1,
                    min_participation_rate: 0.01,
                    max_participation_rate: 0.5,
                },
                vwap: VwapConfig {
                    volume_profile_window_secs: 3600,
                    enable_intraday_curve: true,
                },
                is_algo: IsConfig {
                    risk_aversion: 1.0,
                    impact_model: "almgren_chriss".to_string(),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = SystemConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_market_data_mode() {
        let mut config = SystemConfig::default();
        config.market_data.mode = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_stop_loss_pct() {
        let mut config = SystemConfig::default();
        config.risk.stop_loss_pct = 1.5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_gui_theme() {
        let mut config = SystemConfig::default();
        config.gui.theme = "rainbow".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_pov_participation_rate() {
        let mut config = SystemConfig::default();
        config.algorithms.pov.default_participation_rate = 0.8;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_load_from_string() {
        let toml_str = r#"
[engine]
clock_tick_interval_ms = 100
max_pending_orders = 10000
enable_smart_routing = false
routing_retry_attempts = 3

[market_data]
mode = "rest_api"
websocket_url = "wss://stream.binance.com:9443/ws"
rest_api_url = "https://api.binance.com"
fallback_on_ws_failure = true
ws_reconnect_attempts = 5
tick_sample_interval_ms = 10000
rest_poll_interval_ms = 2000
symbols = ["btcusdt", "ethusdt"]

[risk]
max_position_size = 100000.0
max_total_exposure = 500000.0
stop_loss_pct = 0.02
take_profit_pct = 0.05
enable_risk_checks = true
max_daily_loss = 10000.0

[database]
path = "trading_system.db"
enable_persistence = true
enable_tick_recording = true
auto_vacuum = true

[api]
host = "127.0.0.1"
port = 50051
enable_reflection = true
max_concurrent_streams = 100

[gui]
window_title = "Algorithmic Trading System"
window_width = 1400
window_height = 900
theme = "dark"
chart_update_interval_ms = 500
news_update_interval_ms = 5000
enable_keyboard_shortcuts = true

[algorithms.twap]
default_slice_duration_secs = 60
enable_adaptive = false
adaptive_volatility_window = 20

[algorithms.pov]
default_participation_rate = 0.1
min_participation_rate = 0.01
max_participation_rate = 0.5

[algorithms.vwap]
volume_profile_window_secs = 3600
enable_intraday_curve = true

[algorithms.is_algo]
risk_aversion = 1.0
impact_model = "almgren_chriss"
        "#;

        let config = SystemConfig::from_str(toml_str);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.market_data.mode, "rest_api");
        assert_eq!(config.gui.theme, "dark");
    }
}
