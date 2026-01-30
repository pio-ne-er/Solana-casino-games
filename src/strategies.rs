// Strategy implementations

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

use crate::types::PricePoint;
use crate::config::{StrategyConfig, IndexType};
use crate::indicators::{RollingRSI, RollingMACD, RollingMomentum, calculate_rsi};

/// Trading action decision from strategy
#[derive(Debug, Clone)]
pub enum TradeAction {
    BuyUp {
        price: Decimal,
        shares: Decimal,
    },
    BuyDown {
        price: Decimal,
        shares: Decimal,
    },
    SellUp {
        price: Decimal,
    },
    SellDown {
        price: Decimal,
    },
    NoAction,
}

/// Strategy trait for trading decisions
pub trait Strategy: Send + Sync {
    fn name(&self) -> &str;
    fn config(&self) -> &StrategyConfig;
    fn calculate_index(
        &self,
        prices: &[PricePoint],
        rsi_calc: &RollingRSI,
        macd_calc: &RollingMACD,
        momentum_calc: &RollingMomentum,
    ) -> Option<f64>;
    fn decide(
        &self,
        prices: &[PricePoint],
        rsi_calc: &RollingRSI,
        macd_calc: &RollingMACD,
        momentum_calc: &RollingMomentum,
    ) -> TradeAction;
}

/// Momentum Hedge Strategy implementation
pub struct MomentumHedgeStrategy {
    config: StrategyConfig,
}

impl MomentumHedgeStrategy {
    pub fn new(config: StrategyConfig) -> Self {
        Self { config }
    }
}

impl Strategy for MomentumHedgeStrategy {
    fn name(&self) -> &str {
        // Return a dynamic name based on the index type being used
        match self.config.index_type {
            IndexType::RSI => "MomentumHedgeStrategy (RSI)",
            IndexType::MACD => "MomentumHedgeStrategy (MACD)",
            IndexType::MACDSignal => "MomentumHedgeStrategy (MACD Signal)",
            IndexType::Momentum => "MomentumHedgeStrategy (Momentum)",
        }
    }

    fn config(&self) -> &StrategyConfig {
        &self.config
    }

    fn calculate_index(
        &self,
        prices: &[PricePoint],
        rsi_calc: &RollingRSI,
        macd_calc: &RollingMACD,
        momentum_calc: &RollingMomentum,
    ) -> Option<f64> {
        if prices.len() < self.config.lookback {
            return None;
        }

        match self.config.index_type {
            IndexType::RSI => {
                if rsi_calc.is_ready() {
                    rsi_calc.get_rsi()
                } else {
                    let price_slice: Vec<f64> = prices
                        .iter()
                        .rev()
                        .take(self.config.lookback + 1)
                        .map(|p| p.up_price)
                        .collect();
                    if price_slice.len() >= self.config.lookback + 1 {
                        calculate_rsi(&price_slice, self.config.lookback)
                    } else {
                        None
                    }
                }
            }
            IndexType::MACD => {
                if macd_calc.is_ready() {
                    macd_calc.get_macd()
                } else {
                    None
                }
            }
            IndexType::MACDSignal => {
                // For signal line mode, return MACD value (signal line checked separately)
                if macd_calc.is_ready() {
                    macd_calc.get_macd()
                } else {
                    None
                }
            }
            IndexType::Momentum => {
                if momentum_calc.is_ready() {
                    momentum_calc.get_momentum()
                } else {
                    None
                }
            }
        }
    }

    fn decide(
        &self,
        prices: &[PricePoint],
        rsi_calc: &RollingRSI,
        macd_calc: &RollingMACD,
        momentum_calc: &RollingMomentum,
    ) -> TradeAction {
        if prices.is_empty() || prices.len() < self.config.lookback {
            return TradeAction::NoAction;
        }

        // Calculate trending index for Up token
        let up_index = self.calculate_index(prices, rsi_calc, macd_calc, momentum_calc);
        
        // Calculate trending index for Down token
        let down_prices: Vec<f64> = prices.iter().map(|p| p.down_price).collect();
        let mut temp_rsi_calc_down = RollingRSI::new(self.config.lookback);
        let mut temp_macd_calc_down = RollingMACD::new(self.config.macd_fast_period, self.config.macd_slow_period);
        let mut temp_momentum_calc_down = RollingMomentum::new(self.config.lookback);
        
        for &down_price in &down_prices {
            temp_rsi_calc_down.add_price(down_price);
            temp_macd_calc_down.add_price(down_price);
            temp_momentum_calc_down.add_price(down_price);
        }
        
        let down_index = match self.config.index_type {
            IndexType::RSI => {
                if temp_rsi_calc_down.is_ready() {
                    temp_rsi_calc_down.get_rsi()
                } else {
                    let price_slice: Vec<f64> = prices
                        .iter()
                        .rev()
                        .take(self.config.lookback + 1)
                        .map(|p| p.down_price)
                        .collect();
                    if price_slice.len() >= self.config.lookback + 1 {
                        calculate_rsi(&price_slice, self.config.lookback)
                    } else {
                        None
                    }
                }
            }
            IndexType::MACD => {
                if temp_macd_calc_down.is_ready() {
                    temp_macd_calc_down.get_macd()
                } else {
                    None
                }
            }
            IndexType::MACDSignal => {
                if temp_macd_calc_down.is_ready() {
                    temp_macd_calc_down.get_macd()
                } else {
                    None
                }
            }
            IndexType::Momentum => {
                if temp_momentum_calc_down.is_ready() {
                    temp_momentum_calc_down.get_momentum()
                } else {
                    None
                }
            }
        };

        // Determine which token meets the condition
        // Note: For MACDSignal mode, crossover detection is handled in simulation.rs and trading.rs
        // This method will not be used for entry decisions in MACDSignal mode
        let up_trending = match up_index {
            Some(index) => {
                match self.config.index_type {
                    IndexType::RSI => index > self.config.trend_threshold,
                    IndexType::MACD => index > self.config.trend_threshold,
                    IndexType::MACDSignal => false, // Crossover handled elsewhere
                    IndexType::Momentum => index > self.config.momentum_threshold_pct,
                }
            }
            None => false,
        };

        let down_trending = match down_index {
            Some(index) => {
                match self.config.index_type {
                    IndexType::RSI => index > self.config.trend_threshold,
                    IndexType::MACD => index > self.config.trend_threshold,
                    IndexType::MACDSignal => false, // Crossover handled elsewhere
                    IndexType::Momentum => index > self.config.momentum_threshold_pct,
                }
            }
            None => false,
        };

        // Trade the token that meets the condition
        if up_trending {
            let current_price = Decimal::try_from(prices.last().unwrap().up_price)
                .unwrap_or(dec!(0.0));
            return TradeAction::BuyUp {
                price: current_price,
                shares: self.config.position_size_shares,
            };
        } else if down_trending {
            let current_price = Decimal::try_from(prices.last().unwrap().down_price)
                .unwrap_or(dec!(0.0));
            return TradeAction::BuyDown {
                price: current_price,
                shares: self.config.position_size_shares,
            };
        }

        TradeAction::NoAction
    }
}
