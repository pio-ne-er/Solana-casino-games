// Configuration structures for strategies and execution modes

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use serde::{Serialize, Serializer, Deserialize};
use clap::Parser;
use std::path::PathBuf;
use std::fs;

/// Execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Simulation, // Simulation mode - logs and calculations only
    Live,       // Real-time trading - monitoring and sending real orders
}

/// Index type for trend detection (RSI, MACD, Momentum, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IndexType {
    RSI,
    MACD,
    MACDSignal,  // MACD with Signal Line crossover strategy
    Momentum,
}

/// Strategy configuration
#[derive(Debug, Clone, Serialize)]
pub struct StrategyConfig {
    pub trend_threshold: f64,
    #[serde(serialize_with = "serialize_decimal")]
    pub profit_threshold: Decimal,
    #[serde(serialize_with = "serialize_decimal")]
    pub sl_threshold: Decimal,
    pub lookback: usize,
    pub index_type: IndexType,
    #[serde(serialize_with = "serialize_decimal")]
    pub position_size_shares: Decimal,
    pub macd_fast_period: usize,
    pub macd_slow_period: usize,
    pub macd_signal_period: usize,  // Signal line period (default: 9)
    pub momentum_threshold_pct: f64,
    pub use_macd_sl_filter: bool,
    pub trading_start_when_remaining_minutes: Option<u64>,
}

/// Helper function to serialize Decimal as f64
pub fn serialize_decimal<S>(decimal: &Decimal, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_f64(decimal.to_f64().unwrap_or(0.0))
}

impl StrategyConfig {
    pub fn default_rsi() -> Self {
        Self {
            trend_threshold: 90.0,
            profit_threshold: dec!(0.02),
            sl_threshold: dec!(0.02),
            lookback: 10,
            index_type: IndexType::RSI,
            position_size_shares: dec!(10.0),
            macd_fast_period: 12,
            macd_slow_period: 26,
            macd_signal_period: 9,
            momentum_threshold_pct: 2.0,
            use_macd_sl_filter: false,
            trading_start_when_remaining_minutes: None,
        }
    }

    pub fn default_macd() -> Self {
        Self {
            trend_threshold: 0.0,
            profit_threshold: dec!(0.05),
            sl_threshold: dec!(0.05),
            lookback: 26,
            index_type: IndexType::MACD,
            position_size_shares: dec!(10.0),
            macd_fast_period: 12,
            macd_slow_period: 26,
            macd_signal_period: 9,
            momentum_threshold_pct: 2.0,
            use_macd_sl_filter: true,
            trading_start_when_remaining_minutes: None,
        }
    }

    pub fn default_macd_signal() -> Self {
        Self {
            trend_threshold: 0.0,
            profit_threshold: dec!(0.05),
            sl_threshold: dec!(0.05),
            lookback: 26,
            index_type: IndexType::MACDSignal,
            position_size_shares: dec!(10.0),
            macd_fast_period: 12,
            macd_slow_period: 26,
            macd_signal_period: 9,
            momentum_threshold_pct: 2.0,
            use_macd_sl_filter: false,
            trading_start_when_remaining_minutes: None,
        }
    }

    pub fn default_momentum() -> Self {
        Self {
            trend_threshold: 0.0,
            profit_threshold: dec!(0.05),
            sl_threshold: dec!(0.05),
            lookback: 10,
            index_type: IndexType::Momentum,
            position_size_shares: dec!(10.0),
            macd_fast_period: 12,
            macd_slow_period: 26,
            macd_signal_period: 9,
            momentum_threshold_pct: 2.0,
            use_macd_sl_filter: false,
            trading_start_when_remaining_minutes: None,
        }
    }
}

/// CLI Configuration
#[derive(Parser, Debug)]
#[command(name = "trending-index-trader")]
#[command(about = "Real-time trading bot using trending index strategies")]
pub struct CliConfig {
    /// Strategy type (rsi, macd, momentum)
    #[arg(long, default_value = "rsi")]
    pub strategy: String,

    /// Trend threshold for strategy (e.g., 90.0 for RSI)
    #[arg(long)]
    pub trend_threshold: Option<f64>,

    /// Profit threshold (e.g., 0.02 for 2%)
    #[arg(long)]
    pub profit_threshold: Option<f64>,

    /// Stop loss threshold (e.g., 0.02 for 2%)
    #[arg(long)]
    pub sl_threshold: Option<f64>,

    /// Lookback period for indicators
    #[arg(long)]
    pub lookback: Option<usize>,

    /// Position size in shares
    #[arg(long, default_value = "10.0")]
    pub position_size: f64,

    /// Market to trade (eth, btc, solana, xrp, or all)
    #[arg(long, default_value = "all")]
    pub market: String,

    /// Check interval in milliseconds
    #[arg(long, default_value = "5000")]
    pub check_interval_ms: u64,

    /// Initial capital in USD
    #[arg(long, default_value = "1000.0")]
    pub initial_capital: f64,

    /// Run in simulation mode (logs and calculations only, no real trades)
    #[arg(long, default_value_t = true)]
    pub simulation: bool,

    /// Run in live trading mode (monitoring and sending real orders)
    #[arg(long)]
    pub live: bool,

    /// Private key for trading (required for live mode)
    #[arg(long)]
    pub private_key: Option<String>,

    /// API key for Polymarket (optional, can also use POLYMARKET_API_KEY env var)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Gamma API URL
    #[arg(long, default_value = "https://gamma-api.polymarket.com")]
    pub gamma_url: String,

    /// CLOB API URL
    #[arg(long, default_value = "https://clob.polymarket.com")]
    pub clob_url: String,

    /// Configuration file path (JSON format)
    #[arg(long, default_value = "config.json")]
    pub config: PathBuf,
}

/// JSON configuration file structure
#[derive(Debug, Clone, Deserialize)]
pub struct JsonConfig {
    pub polymarket: Option<PolymarketConfig>,
    pub trading: Option<TradingConfigJson>,
    #[serde(rename = "trending_index")]
    pub trending_index: Option<TrendingIndexJson>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketConfig {
    #[serde(rename = "gamma_api_url")]
    pub gamma_api_url: Option<String>,
    #[serde(rename = "clob_api_url")]
    pub clob_api_url: Option<String>,
    #[serde(rename = "api_key")]
    pub api_key: Option<String>,
    #[serde(rename = "api_secret")]
    pub api_secret: Option<String>,
    #[serde(rename = "api_passphrase")]
    pub api_passphrase: Option<String>,
    #[serde(rename = "private_key")]
    pub private_key: Option<String>,
    #[serde(rename = "proxy_wallet_address")]
    pub proxy_wallet_address: Option<String>,
    #[serde(rename = "signature_type")]
    pub signature_type: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradingConfigJson {
    #[serde(rename = "check_interval_ms")]
    pub check_interval_ms: Option<u64>,
    #[serde(rename = "enable_eth_trading")]
    pub enable_eth_trading: Option<bool>,
    #[serde(rename = "enable_solana_trading")]
    pub enable_solana_trading: Option<bool>,
    #[serde(rename = "enable_xrp_trading")]
    pub enable_xrp_trading: Option<bool>,
    #[serde(rename = "position_size")]
    pub position_size: Option<f64>,
    #[serde(rename = "profit_threshold")]
    pub profit_threshold: Option<f64>,
    #[serde(rename = "stop_loss_threshold")]
    pub stop_loss_threshold: Option<f64>,
    /// Start trading when remaining time is <= this value (in minutes)
    /// For example, if set to 10, trading starts when 10 minutes or less remain in the market
    #[serde(rename = "trading_start_when_remaining_minutes")]
    pub trading_start_when_remaining_minutes: Option<u64>,
}

/// Trending index configuration (strategy + threshold) from config.json
#[derive(Debug, Clone, Deserialize)]
pub struct TrendingIndexJson {
    /// Mode: "rsi", "macd", "momentum"
    #[serde(rename = "mode")]
    pub mode: Option<String>,
    /// Threshold, e.g. 70 for RSI
    #[serde(rename = "threshold")]
    pub threshold: Option<f64>,
    /// Lookback period for indicators
    #[serde(rename = "lookback")]
    pub lookback: Option<usize>,
    /// MACD fast period (default: 12)
    #[serde(rename = "macd_fast_period")]
    pub macd_fast_period: Option<usize>,
    /// MACD slow period (default: 26)
    #[serde(rename = "macd_slow_period")]
    pub macd_slow_period: Option<usize>,
    /// MACD signal line period (default: 9, used for MACDSignal mode)
    #[serde(rename = "macd_signal_period")]
    pub macd_signal_period: Option<usize>,
    /// Use MACD filter for stop loss (only trigger SL if MACD <= 0)
    #[serde(rename = "use_macd_sl_filter")]
    pub use_macd_sl_filter: Option<bool>,
}

impl CliConfig {
    /// Load configuration from JSON file
    pub fn load_json_config(&self) -> Result<JsonConfig, String> {
        let config_path = &self.config;
        
        if !config_path.exists() {
            return Ok(JsonConfig { polymarket: None, trading: None, trending_index: None });
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config file {}: {}", config_path.display(), e))?;
        
        let json_config: JsonConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config file {}: {}", config_path.display(), e))?;
        
        Ok(json_config)
    }

    /// Get API key from CLI arg, config file, or environment variable (in that order)
    pub fn get_api_key(&self) -> Option<String> {
        self.api_key.clone()
            .or_else(|| {
                self.load_json_config().ok()
                    .and_then(|cfg| cfg.polymarket?.api_key)
            })
            .or_else(|| std::env::var("POLYMARKET_API_KEY").ok())
    }

    /// Get API secret from config file or environment variable
    pub fn get_api_secret(&self) -> Option<String> {
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.api_secret)
            .or_else(|| std::env::var("POLYMARKET_API_SECRET").ok())
    }

    /// Get API passphrase from config file or environment variable
    pub fn get_api_passphrase(&self) -> Option<String> {
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.api_passphrase)
            .or_else(|| std::env::var("POLYMARKET_API_PASSPHRASE").ok())
    }

    /// Get private key from CLI arg, config file, or environment variable (in that order)
    pub fn get_private_key(&self) -> Option<String> {
        self.private_key.clone()
            .or_else(|| {
                self.load_json_config().ok()
                    .and_then(|cfg| cfg.polymarket?.private_key)
            })
            .or_else(|| std::env::var("POLYMARKET_PRIVATE_KEY").ok())
    }

    /// Get proxy wallet address from config file or environment variable
    pub fn get_proxy_wallet_address(&self) -> Option<String> {
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.proxy_wallet_address)
            .or_else(|| std::env::var("POLYMARKET_PROXY_WALLET_ADDRESS").ok())
    }

    /// Get signature type from config file (defaults to 0 = EOA)
    pub fn get_signature_type(&self) -> Option<u8> {
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.signature_type)
    }

    /// Get gamma API URL from CLI arg or config file (with default fallback)
    pub fn get_gamma_url(&self) -> String {
        if self.gamma_url != "https://gamma-api.polymarket.com" {
            return self.gamma_url.clone();
        }
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.gamma_api_url)
            .unwrap_or_else(|| "https://gamma-api.polymarket.com".to_string())
    }

    /// Get CLOB API URL from CLI arg or config file (with default fallback)
    pub fn get_clob_url(&self) -> String {
        if self.clob_url != "https://clob.polymarket.com" {
            return self.clob_url.clone();
        }
        self.load_json_config().ok()
            .and_then(|cfg| cfg.polymarket?.clob_api_url)
            .unwrap_or_else(|| "https://clob.polymarket.com".to_string())
    }

    /// Get check interval in milliseconds from CLI or config.json (with default 5000ms)
    pub fn get_check_interval_ms(&self) -> u64 {
        // If user passed CLI value different from default, prefer it
        if self.check_interval_ms != 5000 {
            return self.check_interval_ms;
        }
        // Otherwise, try JSON trading.check_interval_ms
        self.load_json_config()
            .ok()
            .and_then(|cfg| cfg.trading?.check_interval_ms)
            .unwrap_or(5000)
    }

    /// Whether ETH trading is enabled (default true)
    pub fn is_eth_enabled(&self) -> bool {
        self.load_json_config()
            .ok()
            .and_then(|cfg| cfg.trading?.enable_eth_trading)
            .unwrap_or(true)
    }

    /// Whether Solana trading is enabled (default true)
    pub fn is_solana_enabled(&self) -> bool {
        self.load_json_config()
            .ok()
            .and_then(|cfg| cfg.trading?.enable_solana_trading)
            .unwrap_or(true)
    }

    /// Whether XRP trading is enabled (default true)
    pub fn is_xrp_enabled(&self) -> bool {
        self.load_json_config()
            .ok()
            .and_then(|cfg| cfg.trading?.enable_xrp_trading)
            .unwrap_or(true)
    }

    /// Get execution mode
    pub fn mode(&self) -> Mode {
        if self.live {
            Mode::Live
        } else {
            Mode::Simulation
        }
    }

    /// Get strategy configuration
    pub fn get_strategy_config(&self) -> StrategyConfig {
        // Load JSON once so we can use it for multiple fields
        let json_cfg = self.load_json_config().ok();
        let trading_cfg = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trading.as_ref());

        // Determine effective strategy name:
        // 1) If CLI --strategy is not the default "rsi", use it.
        // 2) Else, if config.json.trending_index.mode is set, use that.
        // 3) Else, default to "rsi".
        let cli_strategy = self.strategy.to_lowercase();
        let strategy_name = if cli_strategy != "rsi" {
            cli_strategy
        } else {
            json_cfg
                .as_ref()
                .and_then(|cfg| cfg.trending_index.as_ref())
                .and_then(|ti| ti.mode.as_ref())
                .map(|s| s.to_lowercase())
                .unwrap_or(cli_strategy)
        };

        let mut config = match strategy_name.as_str() {
            "rsi" => StrategyConfig::default_rsi(),
            "macd" => StrategyConfig::default_macd(),
            "macd_signal" => StrategyConfig::default_macd_signal(),
            "momentum" => StrategyConfig::default_momentum(),
            _ => StrategyConfig::default_rsi(),
        };

        // Trend threshold:
        // 1) CLI --trend-threshold if provided
        // 2) config.json.trending_index.threshold if provided
        if let Some(threshold) = self.trend_threshold
            .or_else(|| {
                json_cfg
                    .as_ref()
                    .and_then(|cfg| cfg.trending_index.as_ref())
                    .and_then(|ti| ti.threshold)
            })
        {
            config.trend_threshold = threshold;
        }

        // Profit threshold:
        // 1) CLI --profit-threshold
        // 2) trading.profit_threshold from config.json
        if let Some(profit) = self.profit_threshold
            .or_else(|| trading_cfg.and_then(|t| t.profit_threshold))
        {
            config.profit_threshold = Decimal::try_from(profit)
                .unwrap_or(config.profit_threshold);
        }

        // Stop loss threshold:
        // 1) CLI --sl-threshold
        // 2) trading.stop_loss_threshold from config.json
        if let Some(sl) = self.sl_threshold
            .or_else(|| trading_cfg.and_then(|t| t.stop_loss_threshold))
        {
            config.sl_threshold = Decimal::try_from(sl)
                .unwrap_or(config.sl_threshold);
        }

        // Position size (shares):
        // 1) CLI --position-size if it differs from default 10.0
        // 2) trading.position_size from config.json
        // 3) default from underlying StrategyConfig
        let pos_size = if (self.position_size - 10.0).abs() > f64::EPSILON {
            self.position_size
        } else {
            trading_cfg
                .and_then(|t| t.position_size)
                .unwrap_or(self.position_size)
        };
        config.position_size_shares = Decimal::try_from(pos_size)
            .unwrap_or(config.position_size_shares);
        if let Some(profit) = self.profit_threshold {
            config.profit_threshold = Decimal::try_from(profit).unwrap_or(config.profit_threshold);
        }
        if let Some(sl) = self.sl_threshold {
            config.sl_threshold = Decimal::try_from(sl).unwrap_or(config.sl_threshold);
        }
        // Lookback period:
        // 1) CLI --lookback if provided
        // 2) config.json.trending_index.lookback if provided
        if let Some(lookback) = self.lookback
            .or_else(|| {
                json_cfg
                    .as_ref()
                    .and_then(|cfg| cfg.trending_index.as_ref())
                    .and_then(|ti| ti.lookback)
            })
        {
            config.lookback = lookback;
        }

        // MACD fast period:
        // config.json.trending_index.macd_fast_period if provided
        if let Some(fast_period) = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trending_index.as_ref())
            .and_then(|ti| ti.macd_fast_period)
        {
            config.macd_fast_period = fast_period;
        }

        // MACD slow period:
        // config.json.trending_index.macd_slow_period if provided
        if let Some(slow_period) = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trending_index.as_ref())
            .and_then(|ti| ti.macd_slow_period)
        {
            config.macd_slow_period = slow_period;
        }

        // MACD signal period:
        // config.json.trending_index.macd_signal_period if provided
        if let Some(signal_period) = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trending_index.as_ref())
            .and_then(|ti| ti.macd_signal_period)
        {
            config.macd_signal_period = signal_period;
        }

        // MACD SL filter:
        // config.json.trending_index.use_macd_sl_filter if provided
        if let Some(use_filter) = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trending_index.as_ref())
            .and_then(|ti| ti.use_macd_sl_filter)
        {
            config.use_macd_sl_filter = use_filter;
        }

        // Trading start delay:
        // config.json.trading.trading_start_when_remaining_minutes if provided
        if let Some(remaining_minutes) = json_cfg
            .as_ref()
            .and_then(|cfg| cfg.trading.as_ref())
            .and_then(|t| t.trading_start_when_remaining_minutes)
        {
            config.trading_start_when_remaining_minutes = Some(remaining_minutes);
        }

        config
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.mode() == Mode::Live && self.get_private_key().is_none() {
            return Err("Private key required for live trading mode. Set POLYMARKET_PRIVATE_KEY environment variable or use --private-key".to_string());
        }
        Ok(())
    }
}
