// Core types used throughout the trading system

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use serde::Serialize;

/// Price data point with both Up and Down token prices
#[derive(Debug, Clone)]
pub struct PricePoint {
    pub timestamp: u64,
    pub up_price: f64,   // Up token price
    pub down_price: f64, // Down token price
    pub actual_outcome: Option<u8>, // 1 for Up win, 0 for Down win, None if not specified
    pub asset: Option<String>, // Asset identifier (e.g., "BTC", "ETH")
    pub news_event: Option<i8>, // 1 for positive Up news, -1 for Down, 0 for none
}

/// Simple side enum for active trading cycles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Flat,
    LongUp,
    LongDown,
}

/// One active trading cycle (entry + TP + SL levels)
///
/// This is used by both simulation and live traders to ensure we only
/// have exactly one open position per asset at a time, matching the
/// backtest behavior: buy -> TP or SL hit -> flat again.
#[derive(Debug, Clone)]
pub struct ActiveCycle {
    pub side: PositionSide,
    /// Entry price of the token we bought
    pub entry_price: Decimal,
    /// Position size in tokens
    pub size: Decimal,
    /// Take‑profit limit price
    pub tp_price: Decimal,
    /// Stop‑loss limit price
    pub sl_price: Decimal,
}

impl PricePoint {
    /// Get price for backward compatibility (returns Up price)
    pub fn price(&self) -> f64 {
        self.up_price
    }
}

/// Position state during trading
#[derive(Debug, Clone)]
pub enum PositionState {
    NoPosition,
    LongUp {
        buy_price: Decimal,
        size: Decimal, // Position size in tokens
        cost: Decimal, // Total cost in USD
    },
    LongDown {
        buy_price: Decimal,
        size: Decimal, // Position size in tokens
        cost: Decimal, // Total cost in USD
    },
    Hedged {
        up_buy_price: Decimal,
        up_size: Decimal,
        up_cost: Decimal,
        down_buy_price: Decimal,
        down_size: Decimal,
        down_cost: Decimal,
    },
}

/// Trading statistics
#[derive(Debug, Default)]
pub struct TradingStats {
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub total_pnl: Decimal,
    pub current_capital: Decimal,
    pub equity_curve: Vec<(u64, Decimal)>, // (timestamp, equity)
}

impl TradingStats {
    pub fn win_rate(&self) -> f64 {
        if self.total_trades == 0 {
            return 0.0;
        }
        self.winning_trades as f64 / self.total_trades as f64 * 100.0
    }

    pub fn add_equity_point(&mut self, timestamp: u64, equity: Decimal) {
        self.equity_curve.push((timestamp, equity));
    }
}

/// Trade log entry
#[derive(Debug, Clone, Serialize)]
pub struct TradeLog {
    pub ts: u64,
    pub action: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub current_capital: Decimal,
    pub pl: Decimal,
    pub asset: Option<String>,
    pub trending_index_name: Option<String>,
    pub trending_index_value: Option<f64>,
}
