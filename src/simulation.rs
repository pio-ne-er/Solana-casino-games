// Simulation mode - logs and calculations only, no real trades

use crate::config::{CliConfig, StrategyConfig, IndexType};
use crate::monitor::{MarketMonitor, MarketSnapshot};
use crate::strategies::{Strategy, TradeAction, MomentumHedgeStrategy};
use crate::types::{PricePoint, TradingStats, ActiveCycle, PositionSide};
use crate::indicators::{RollingRSI, RollingMACD, RollingMomentum};
use rust_decimal::Decimal;
use rust_decimal::prelude::{ToPrimitive, FromPrimitive};
use rust_decimal_macros::dec;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

/// Simulation mode trader - logs and calculations only
pub struct SimulationTrader {
    monitor: Arc<MarketMonitor>,
    strategy: Box<dyn Strategy>,
    price_history: VecDeque<PricePoint>,
    stats: TradingStats,
    capital: Decimal,
    config: CliConfig,
    rsi_calculator: RollingRSI,
    macd_calculator: RollingMACD,
    momentum_calculator: RollingMomentum,
    trading_assets: Vec<String>,
    /// Current active trading cycle (if any) for the asset being processed
    current_cycle: Option<ActiveCycle>,
    /// Total PnL across all trades (starts at 0, adds profit, subtracts losses)
    total_pnl: Decimal,
    /// Number of winning trades (TP hits)
    wins: usize,
    /// Number of losing trades (SL hits)
    losses: usize,
    /// Total fund used (accumulates entry_price * size for each trade)
    total_fund_used: Decimal,
    /// Previous period timestamp to detect market rollover
    previous_period_timestamp: Option<u64>,
    /// Last price point for each asset (used for final PnL calculation at market end)
    last_price_points: std::collections::HashMap<String, PricePoint>,
    /// Previous MACD value for Up token (for momentum acceleration check)
    previous_macd_up: Option<f64>,
    /// Previous MACD value for Down token (for momentum acceleration check)
    previous_macd_down: Option<f64>,
    /// Previous signal line value for Up token (for MACDSignal crossover detection)
    previous_signal_up: Option<f64>,
    /// Previous signal line value for Down token (for MACDSignal crossover detection)
    previous_signal_down: Option<f64>,
}

impl SimulationTrader {
    pub fn new(
        monitor: Arc<MarketMonitor>,
        strategy_config: StrategyConfig,
        config: CliConfig,
        initial_capital: Decimal,
    ) -> Self {
        // Decide which assets to trade based on CLI `--market` and config.json enable_* flags
        let mut trading_assets = match config.market.to_lowercase().as_str() {
            "eth" => vec!["ETH".to_string()],
            "btc" => vec!["BTC".to_string()],
            "sol" | "solana" => vec!["SOL".to_string()],
            "xrp" => vec!["XRP".to_string()],
            _ => vec!["ETH".to_string(), "BTC".to_string(), "SOL".to_string(), "XRP".to_string()],
        };

        // Apply enable_* flags from config.json (like polymarket-trading-bot)
        trading_assets.retain(|asset| match asset.as_str() {
            "ETH" => config.is_eth_enabled(),
            "SOL" => config.is_solana_enabled(),
            "XRP" => config.is_xrp_enabled(),
            _ => true, // BTC always allowed (there's no enable_btc_trading in the JSON)
        });

        // Create MACD calculator with or without signal line based on index type
        let macd_calculator = if strategy_config.index_type == IndexType::MACDSignal {
            RollingMACD::new_with_signal(
                strategy_config.macd_fast_period,
                strategy_config.macd_slow_period,
                strategy_config.macd_signal_period,
            )
        } else {
            RollingMACD::new(
                strategy_config.macd_fast_period,
                strategy_config.macd_slow_period,
            )
        };

        Self {
            monitor,
            strategy: Box::new(MomentumHedgeStrategy::new(strategy_config.clone())),
            price_history: VecDeque::new(),
            stats: TradingStats::default(),
            capital: initial_capital,
            config,
            rsi_calculator: RollingRSI::new(strategy_config.lookback),
            macd_calculator,
            momentum_calculator: RollingMomentum::new(strategy_config.lookback),
            trading_assets,
            current_cycle: None,
            total_pnl: Decimal::ZERO,
            wins: 0,
            losses: 0,
            total_fund_used: Decimal::ZERO,
            previous_period_timestamp: None,
            last_price_points: std::collections::HashMap::new(),
            previous_macd_up: None,
            previous_macd_down: None,
            previous_signal_up: None,
            previous_signal_down: None,
        }
    }

    /// Convert MarketSnapshot to PricePoint
    fn snapshot_to_price_point(snapshot: &MarketSnapshot, asset: &str) -> Option<PricePoint> {
        let market_data = match asset {
            "ETH" => &snapshot.eth_market,
            "BTC" => &snapshot.btc_market,
            _ => return None,
        };

        let up_price = market_data.up_token.as_ref()
            .and_then(|t| t.ask_price().to_f64())
            .unwrap_or(0.0);

        let down_price = market_data.down_token.as_ref()
            .and_then(|t| t.ask_price().to_f64())
            .unwrap_or(0.0);

        Some(PricePoint {
            timestamp: snapshot.period_timestamp,
            up_price,
            down_price,
            actual_outcome: None,
            asset: Some(asset.to_string()),
            news_event: None,
        })
    }

    /// Handle market end: calculate final PnL for open positions and log summary
    fn handle_market_end(&mut self, asset: &str) {
        // First, handle any open cycle at market end
        if let Some(cycle) = &self.current_cycle {
            // Get final prices from the last price point of the old market
            if let Some(price_point) = self.last_price_points.get(asset) {
                // Determine market outcome: Up wins if up_price = 1.0, Down wins if down_price = 1.0
                let market_outcome_up = price_point.up_price >= 0.99; // Up token won (price ≈ 1.0)
                let market_outcome_down = price_point.down_price >= 0.99; // Down token won (price ≈ 1.0)
                
                let (final_pnl, is_win) = match cycle.side {
                    PositionSide::LongUp => {
                        if market_outcome_up {
                            // We bought Up, Up won: PnL = (1.0 - entry) * size
                            let pnl = (Decimal::ONE - cycle.entry_price) * cycle.size;
                            (pnl, true)
                        } else {
                            // We bought Up, Down won: PnL = (0.0 - entry) * size
                            let pnl = (Decimal::ZERO - cycle.entry_price) * cycle.size;
                            (pnl, false)
                        }
                    }
                    PositionSide::LongDown => {
                        if market_outcome_down {
                            // We bought Down, Down won: PnL = (1.0 - entry) * size
                            let pnl = (Decimal::ONE - cycle.entry_price) * cycle.size;
                            (pnl, true)
                        } else {
                            // We bought Down, Up won: PnL = (0.0 - entry) * size
                            let pnl = (Decimal::ZERO - cycle.entry_price) * cycle.size;
                            (pnl, false)
                        }
                    }
                    PositionSide::Flat => (Decimal::ZERO, false),
                };
                
                // Update statistics
                self.total_pnl += final_pnl;
                if is_win {
                    self.wins += 1;
                } else {
                    self.losses += 1;
                }
                
                let outcome_str = if market_outcome_up { "UP" } else { "DOWN" };
                let msg = format!(
                    "[SIM] 🏁 MARKET END | asset={} | side={:?} | entry={:.4} | outcome={} | pnl={:.4} | {}",
                    asset, cycle.side, cycle.entry_price, outcome_str, final_pnl,
                    if is_win { "WIN" } else { "LOSS" }
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                
                // Close the cycle
                self.current_cycle = None;
            }
        }
        
        // ALWAYS log final summary for this market (even if no trades occurred)
        let summary_msg = format!(
            "[SIM] 📊 MARKET SUMMARY | asset={} | total_pnl={:.4} | wins={} | losses={} | fund_used={:.4}",
            asset, self.total_pnl, self.wins, self.losses, self.total_fund_used
        );
        println!("{}", summary_msg);
        crate::log_trading_event(&summary_msg);
    }

    /// Reset indicators and price history for a new market
    fn reset_indicators_for_new_market(&mut self) {
        let cfg = self.strategy.config();
        // Reset all indicators to start fresh for the new market
        self.rsi_calculator = RollingRSI::new(cfg.lookback);
        // Create MACD calculator with or without signal line based on index type
        self.macd_calculator = if cfg.index_type == IndexType::MACDSignal {
            RollingMACD::new_with_signal(
                cfg.macd_fast_period,
                cfg.macd_slow_period,
                cfg.macd_signal_period,
            )
        } else {
            RollingMACD::new(cfg.macd_fast_period, cfg.macd_slow_period)
        };
        self.momentum_calculator = RollingMomentum::new(cfg.lookback);
        // Reset previous MACD and signal line values when starting new market
        self.previous_macd_up = None;
        self.previous_macd_down = None;
        self.previous_signal_up = None;
        self.previous_signal_down = None;
        // Clear price history so indicators build up from scratch
        self.price_history.clear();
        // Clear last known prices from old market
        self.last_price_points.clear();
        let reset_msg = "[SIM] 🔄 NEW MARKET | Resetting indicators and price history";
        println!("{}", reset_msg);
        crate::log_trading_event(reset_msg);
    }

    /// Reset per-market performance counters (wins/losses/pnl/fund) back to 0.
    fn reset_market_stats(&mut self) {
        self.total_pnl = Decimal::ZERO;
        self.wins = 0;
        self.losses = 0;
        self.total_fund_used = Decimal::ZERO;

        let msg = "[SIM] 🔁 NEW MARKET | Resetting market stats (pnl/wins/losses/fund)";
        println!("{}", msg);
        crate::log_trading_event(msg);
    }

    /// Process a snapshot and make trading decisions
    async fn process_snapshot(&mut self, snapshot: &MarketSnapshot) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Check if market period changed (market ended)
        let current_period = snapshot.period_timestamp;
        if let Some(prev_period) = self.previous_period_timestamp {
            if prev_period != current_period {
                // Market ended - FIRST log a separator, then handle final PnL for each asset
                let separator_msg = "[SIM] ════════════════════════════════════════════════════════════════════════════";
                println!("{}", separator_msg);
                crate::log_trading_event(separator_msg);
                
                let assets = self.trading_assets.clone();
                for asset in &assets {
                    self.handle_market_end(asset);
                }
                
                // Log another separator after all summaries
                let separator_end = "[SIM] ════════════════════════════════════════════════════════════════════════════";
                println!("{}", separator_end);
                crate::log_trading_event(separator_end);
                
                // Reset indicators and price history for the new market
                self.reset_indicators_for_new_market();
                // Reset per-market stats so new market starts from 0
                self.reset_market_stats();
            }
        }
        self.previous_period_timestamp = Some(current_period);
        
        let assets = self.trading_assets.clone();
        for asset in &assets {
            if let Some(price_point) = Self::snapshot_to_price_point(snapshot, asset) {
                // Store the last price point for this asset (for final PnL calculation)
                self.last_price_points.insert(asset.clone(), price_point.clone());
                self.process_price_point(&price_point).await?;
            }
        }
        Ok(())
    }

    /// Process a single price point
    async fn process_price_point(&mut self, price_point: &PricePoint) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.price_history.push_back(price_point.clone());
        
        if self.price_history.len() > 100 {
            self.price_history.pop_front();
        }

        let prices: Vec<PricePoint> = self.price_history.iter().cloned().collect();

        // Update indicators (Up token)
        if let Some(up_price) = prices.last().map(|p| p.up_price) {
            self.rsi_calculator.add_price(up_price);
            self.macd_calculator.add_price(up_price);
            self.momentum_calculator.add_price(up_price);
        }

        // Compute trending indices for Up and Down tokens
        let cfg = self.strategy.config().clone();
        let up_index = self
            .strategy
            .calculate_index(&prices, &self.rsi_calculator, &self.macd_calculator, &self.momentum_calculator);

        // Build temporary calculators for Down token to compute its index
        let (down_index, down_signal) = if prices.len() >= cfg.lookback {
            let mut rsi_down = RollingRSI::new(cfg.lookback);
            // Create MACD calculator with or without signal line based on index type
            let mut macd_down = if cfg.index_type == IndexType::MACDSignal {
                RollingMACD::new_with_signal(
                    cfg.macd_fast_period,
                    cfg.macd_slow_period,
                    cfg.macd_signal_period,
                )
            } else {
                RollingMACD::new(cfg.macd_fast_period, cfg.macd_slow_period)
            };
            let mut mom_down = RollingMomentum::new(cfg.lookback);
            for p in &prices {
                let dp = p.down_price;
                rsi_down.add_price(dp);
                macd_down.add_price(dp);
                mom_down.add_price(dp);
            }
            let index = match cfg.index_type {
                IndexType::RSI => {
                    if rsi_down.is_ready() {
                        rsi_down.get_rsi()
                    } else {
                        None
                    }
                }
                IndexType::MACD => {
                    if macd_down.is_ready() {
                        macd_down.get_macd()
                    } else {
                        None
                    }
                }
                IndexType::MACDSignal => {
                    if macd_down.is_ready() {
                        macd_down.get_macd()
                    } else {
                        None
                    }
                }
                IndexType::Momentum => {
                    if mom_down.is_ready() {
                        mom_down.get_momentum()
                    } else {
                        None
                    }
                }
            };
            let signal = if cfg.index_type == IndexType::MACDSignal {
                if macd_down.is_signal_ready() {
                    macd_down.get_signal_line()
                } else {
                    None
                }
            } else {
                None
            };
            (index, signal)
        } else {
            (None, None)
        };

        // For MACDSignal mode: Get signal line values for Up token
        let up_signal = if cfg.index_type == IndexType::MACDSignal {
            if self.macd_calculator.is_signal_ready() {
                self.macd_calculator.get_signal_line()
            } else {
                None
            }
        } else {
            None
        };

        // For MACD mode: Check if MACD is increasing (momentum acceleration)
        // Only allow trades if MACD is both above threshold AND increasing
        // Store previous values before updating (for logging purposes)
        let prev_macd_up_for_log = self.previous_macd_up;
        let prev_macd_down_for_log = self.previous_macd_down;
        
        let macd_increasing_check = if cfg.index_type == IndexType::MACD {
            let up_macd_increasing = match (up_index, self.previous_macd_up) {
                (Some(current), Some(previous)) => current > previous, // MACD is increasing
                (Some(_), None) => true, // First MACD value, allow it (no previous to compare)
                _ => false, // No current MACD value
            };
            
            let down_macd_increasing = match (down_index, self.previous_macd_down) {
                (Some(current), Some(previous)) => current > previous, // MACD is increasing
                (Some(_), None) => true, // First MACD value, allow it (no previous to compare)
                _ => false, // No current MACD value
            };
            
            (up_macd_increasing, down_macd_increasing)
        } else {
            (true, true) // Not MACD mode, skip the check
        };
        
        // Helper: current asset name (for logs)
        let asset = price_point
            .asset
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string());

        // For MACDSignal mode: Detect crossovers (MACD crosses above Signal Line)
        let mut action = if cfg.index_type == IndexType::MACDSignal {
            // Check for Up token crossover
            let up_crosses_above_signal = match (up_index, up_signal, self.previous_macd_up, self.previous_signal_up) {
                (Some(current_macd), Some(current_signal), Some(prev_macd), Some(prev_signal)) => {
                    // Crossover: previous MACD <= previous Signal AND current MACD > current Signal
                    prev_macd <= prev_signal && current_macd > current_signal
                }
                (Some(current_macd), Some(current_signal), None, None) => {
                    // First values: if MACD > Signal, consider it a crossover
                    current_macd > current_signal
                }
                _ => false,
            };
            
            // Check for Down token crossover
            let down_crosses_above_signal = match (down_index, down_signal, self.previous_macd_down, self.previous_signal_down) {
                (Some(current_macd), Some(current_signal), Some(prev_macd), Some(prev_signal)) => {
                    // Crossover: previous MACD <= previous Signal AND current MACD > current Signal
                    prev_macd <= prev_signal && current_macd > current_signal
                }
                (Some(current_macd), Some(current_signal), None, None) => {
                    // First values: if MACD > Signal, consider it a crossover
                    current_macd > current_signal
                }
                _ => false,
            };
            
            if up_crosses_above_signal {
                let current_price = Decimal::try_from(price_point.up_price).unwrap_or(dec!(0.0));
                let msg = format!(
                    "[SIM] 🔀 MACD CROSSOVER | asset={} | token=UP | macd={:.4} | signal={:.4} | price={:.4}",
                    asset, up_index.unwrap_or(0.0), up_signal.unwrap_or(0.0), price_point.up_price
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                TradeAction::BuyUp {
                    price: current_price,
                    shares: cfg.position_size_shares,
                }
            } else if down_crosses_above_signal {
                let current_price = Decimal::try_from(price_point.down_price).unwrap_or(dec!(0.0));
                let msg = format!(
                    "[SIM] 🔀 MACD CROSSOVER | asset={} | token=DOWN | macd={:.4} | signal={:.4} | price={:.4}",
                    asset, down_index.unwrap_or(0.0), down_signal.unwrap_or(0.0), price_point.down_price
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                TradeAction::BuyDown {
                    price: current_price,
                    shares: cfg.position_size_shares,
                }
            } else {
                TradeAction::NoAction
            }
        } else {
            // Get strategy decision for non-MACDSignal modes
            self.strategy.decide(
                &prices,
                &self.rsi_calculator,
                &self.macd_calculator,
                &self.momentum_calculator,
            )
        };
        
        // Update previous MACD and signal line values for next iteration
        if cfg.index_type == IndexType::MACD {
            self.previous_macd_up = up_index;
            self.previous_macd_down = down_index;
        } else if cfg.index_type == IndexType::MACDSignal {
            self.previous_macd_up = up_index;
            self.previous_macd_down = down_index;
            self.previous_signal_up = up_signal;
            self.previous_signal_down = down_signal;
        }

        // Choose index name for logging
        let idx_name = match cfg.index_type {
            IndexType::RSI => "RSI",
            IndexType::MACD => "MACD",
            IndexType::MACDSignal => "MACD_SIG",
            IndexType::Momentum => "MOM",
        };

        // Helper: current asset name (for logs)
        let asset = price_point
            .asset
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string());

        // 1) If we already have an open cycle, check TP/SL first
        if let Some(cycle) = self.current_cycle.clone() {
            // TP: Check same token ask price (TP = sell same token at TP)
            let same_token_price_f64 = match cycle.side {
                PositionSide::LongUp => price_point.up_price,
                PositionSide::LongDown => price_point.down_price,
                PositionSide::Flat => 0.0,
            };
            
            // SL: Check opposite token ask price (SL = buy opposite token at (1 - SL))
            let opposite_token_price_f64 = match cycle.side {
                PositionSide::LongUp => price_point.down_price,  // We bought Up, check Down ask price
                PositionSide::LongDown => price_point.up_price,  // We bought Down, check Up ask price
                PositionSide::Flat => 0.0,
            };

            if same_token_price_f64 > 0.0 {
                if let Some(tp_price) = Decimal::from_f64(same_token_price_f64) {
                    // Take‑profit hit (only check if TP is valid, i.e., <= 1.0)
                    if cycle.tp_price <= Decimal::ONE && tp_price >= cycle.tp_price {
                        let pnl = (cycle.tp_price - cycle.entry_price) * cycle.size;
                        // Update statistics (fund was already added when position opened)
                        self.total_pnl += pnl;
                        self.wins += 1;
                        let msg = format!(
                            "[SIM] ✅ TP HIT   | asset={} | side={:?} | entry={:.4} | tp={:.4} | size={:.4} | pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.tp_price,
                            cycle.size,
                            pnl
                        );
                        println!("{}", msg);
                        info!(
                            "[SIM] TP HIT | asset={} side={:?} entry={:.4} tp={:.4} size={:.4} pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.tp_price,
                            cycle.size,
                            pnl
                        );
                        crate::log_trading_event(&msg);
                        // Close cycle
                        self.current_cycle = None;
                    }
                }
            }
            
            // Stop‑loss hit: check if opposite token ask price is at or above (1 - SL)
            // Only check if cycle is still open (TP didn't close it)
            // Note: When same token price drops, opposite token price rises, so condition is reversed (>= instead of <=)
            if self.current_cycle.is_some() && opposite_token_price_f64 > 0.0 {
                let opposite_sl_price = Decimal::ONE - cycle.sl_price;
                if let Some(opposite_token_ask_price) = Decimal::from_f64(opposite_token_price_f64) {
                    // SL hit: opposite token ask price is at or above (1 - SL), meaning same token has dropped to SL
                    let price_sl_hit = opposite_token_ask_price >= opposite_sl_price;
                    
                    // For MACD mode with filter enabled: additional check - only trigger SL if MACD of held token is <= 0
                    let should_trigger_sl = if cfg.index_type == IndexType::MACD && cfg.use_macd_sl_filter {
                        // Get MACD value of the token we're holding
                        let held_token_macd = match cycle.side {
                            PositionSide::LongUp => up_index,
                            PositionSide::LongDown => down_index,
                            PositionSide::Flat => None,
                        };
                        
                        match held_token_macd {
                            Some(macd_value) => {
                                // Only trigger SL if MACD <= 0 (momentum is negative or zero)
                                if macd_value > 0.0 {
                                    // MACD still positive - don't trigger SL
                                    // Only log if price condition was actually met
                                    if price_sl_hit {
                                        let msg = format!(
                                            "[SIM] ⏸️  SL SKIPPED (MACD > 0) | asset={} | side={:?} | MACD={:.4} > 0 | price condition met but momentum still positive",
                                            asset, cycle.side, macd_value
                                        );
                                        println!("{}", msg);
                                        crate::log_trading_event(&msg);
                                    }
                                    false
                                } else {
                                    // MACD <= 0 - trigger SL
                                    true
                                }
                            }
                            None => {
                                // MACD not available - proceed with SL (fallback to price-based SL)
                                true
                            }
                        }
                    } else {
                        // Not MACD mode or filter disabled - use price-based SL only
                        price_sl_hit
                    };
                    
                    if price_sl_hit && should_trigger_sl {
                        // Place BUY order for opposite token at (1 - SL) to execute stop loss (matching live mode)
                        let opposite_sl_price = Decimal::ONE - cycle.sl_price;
                        let opposite_sl_price_rounded = opposite_sl_price.round_dp(2);
                        let opposite_token = match cycle.side {
                            PositionSide::LongUp => "DOWN",
                            PositionSide::LongDown => "UP",
                            PositionSide::Flat => "",
                        };
                        let sl_order_msg = format!(
                            "[SIM] 📌 SL ORDER | side=BUY | asset={} | opposite_token={} | price={:.2} (1-SL={:.2}) | shares={:.2}",
                            asset, opposite_token, opposite_sl_price_rounded, cycle.sl_price, cycle.size
                        );
                        println!("{}", sl_order_msg);
                        crate::log_trading_event(&sl_order_msg);
                        
                        let pnl = (cycle.sl_price - cycle.entry_price) * cycle.size;
                        // Update statistics (fund was already added when position opened)
                        self.total_pnl += pnl;
                        self.losses += 1;
                        let msg = format!(
                            "[SIM] ❌ SL HIT   | asset={} | side={:?} | entry={:.4} | sl={:.4} | opposite_ask={:.4} | target=(1-SL)={:.4} | size={:.4} | pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.sl_price,
                            opposite_token_ask_price,
                            opposite_sl_price,
                            cycle.size,
                            pnl
                        );
                        println!("{}", msg);
                        info!(
                            "[SIM] SL HIT | asset={} side={:?} entry={:.4} sl={:.4} opposite_ask={:.4} target=(1-SL)={:.4} size={:.4} pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.sl_price,
                            opposite_token_ask_price,
                            opposite_sl_price,
                            cycle.size,
                            pnl
                        );
                        crate::log_trading_event(&msg);
                        // Close cycle
                        self.current_cycle = None;
                    }
                }
            }
        }

        // 2) If we are flat (no active cycle) and strategy says BUY, open new cycle
        if self.current_cycle.is_none() {
            // Helper: format Option<f64> indices - 4 decimals for MACD, 2 decimals for others
            let up_idx_str = match (up_index, cfg.index_type) {
                (Some(v), IndexType::MACD) => format!("{:.4}", v),
                (Some(v), _) => format!("{:.2}", v),
                (None, _) => "n/a".to_string(),
            };
            let down_idx_str = match (down_index, cfg.index_type) {
                (Some(v), IndexType::MACD) => format!("{:.4}", v),
                (Some(v), _) => format!("{:.2}", v),
                (None, _) => "n/a".to_string(),
            };

            match &action {
                TradeAction::BuyUp { price, shares } => {
                    // For MACD mode: Check if MACD is increasing (momentum acceleration)
                    if cfg.index_type == IndexType::MACD && !macd_increasing_check.0 {
                        let msg = format!(
                            "[SIM] ⏸️  MACD NOT INCREASING | asset={} | MACD_up={:.4} | previous={:.4} | momentum not accelerating",
                            asset,
                            up_index.unwrap_or(0.0),
                            prev_macd_up_for_log.unwrap_or(0.0)
                        );
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                        return Ok(()); // Skip placing entry order - MACD not increasing
                    }
                    
                    // Check if trading should start based on remaining time
                    if let Some(required_remaining_minutes) = cfg.trading_start_when_remaining_minutes {
                        let current_time = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let period_start = price_point.timestamp;
                        let elapsed_seconds = current_time.saturating_sub(period_start);
                        let market_duration_seconds: u64 = 15 * 60; // 15 minutes = 900 seconds
                        let remaining_seconds = market_duration_seconds.saturating_sub(elapsed_seconds);
                        let remaining_minutes = remaining_seconds / 60;
                        
                        if remaining_minutes > required_remaining_minutes {
                            let msg = format!(
                                "[SIM] ⏸️  TRADING NOT STARTED | asset={} | remaining={}m > {}m | waiting for market to reach {}m remaining",
                                asset, remaining_minutes, required_remaining_minutes, required_remaining_minutes
                            );
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                            return Ok(()); // Skip placing entry order
                        }
                    }
                    
                    // Calculate TP/SL based on config thresholds
                    let entry_price = *price;
                    let size = *shares;
                    // Use absolute thresholds: TP = entry + profit_threshold, SL = entry - sl_threshold
                    let tp_price = entry_price + cfg.profit_threshold;
                    let sl_price = entry_price - cfg.sl_threshold;

                    self.current_cycle = Some(ActiveCycle {
                        side: PositionSide::LongUp,
                        entry_price,
                        size,
                        tp_price,
                        sl_price,
                    });

                    // Update fund used when position opens
                    self.total_fund_used += entry_price * size;

                    let msg = format!(
                        "[SIM] 🟢 BUY UP   | asset={} | shares={} | entry={:.4} | TP={:.4} | SL={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, tp_price, sl_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    println!("{}", msg);
                    info!(
                        "[SIM] 🟢 OPEN CYCLE UP | asset={} | shares={} | entry={:.4} | TP={:.4} | SL={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, tp_price, sl_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    crate::log_trading_event(&msg);
                    // Simulate balance confirmation delay (5 seconds) before placing TP order
                    // In live mode, this delay happens automatically during balance confirmation
                    sleep(Duration::from_secs(5)).await;
                    
                    // TP: Place LIMIT SELL order for same token at TP price (matching live mode)
                    if tp_price <= Decimal::ONE {
                        let tp_price_rounded = tp_price.round_dp(2);
                        let limit_msg = format!(
                            "[SIM] 📌 LIMIT    | side=SELL | asset={} | token=UP | price={:.2} | shares={:.2}",
                            asset, tp_price_rounded, size
                        );
                        println!("{}", limit_msg);
                        crate::log_trading_event(&limit_msg);
                    } else {
                        let wait_msg = format!(
                            "[SIM] ⏸️  NO LIMIT | asset={} | TP={:.4} out of [0,1] | waiting for SL or market end",
                            asset, tp_price
                        );
                        println!("{}", wait_msg);
                        crate::log_trading_event(&wait_msg);
                    }
                }
                TradeAction::BuyDown { price, shares } => {
                    // For MACD mode: Check if MACD is increasing (momentum acceleration)
                    if cfg.index_type == IndexType::MACD && !macd_increasing_check.1 {
                        let msg = format!(
                            "[SIM] ⏸️  MACD NOT INCREASING | asset={} | MACD_down={:.4} | previous={:.4} | momentum not accelerating",
                            asset,
                            down_index.unwrap_or(0.0),
                            prev_macd_down_for_log.unwrap_or(0.0)
                        );
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                        return Ok(()); // Skip placing entry order - MACD not increasing
                    }
                    
                    // Check if trading should start based on remaining time
                    if let Some(required_remaining_minutes) = cfg.trading_start_when_remaining_minutes {
                        let current_time = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let period_start = price_point.timestamp;
                        let elapsed_seconds = current_time.saturating_sub(period_start);
                        let market_duration_seconds: u64 = 15 * 60; // 15 minutes = 900 seconds
                        let remaining_seconds = market_duration_seconds.saturating_sub(elapsed_seconds);
                        let remaining_minutes = remaining_seconds / 60;
                        
                        if remaining_minutes > required_remaining_minutes {
                            let msg = format!(
                                "[SIM] ⏸️  TRADING NOT STARTED | asset={} | remaining={}m > {}m | waiting for market to reach {}m remaining",
                                asset, remaining_minutes, required_remaining_minutes, required_remaining_minutes
                            );
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                            return Ok(()); // Skip placing entry order
                        }
                    }
                    
                    let entry_price = *price;
                    let size = *shares;
                    let tp_price = entry_price + cfg.profit_threshold;
                    let sl_price = entry_price - cfg.sl_threshold;

                    self.current_cycle = Some(ActiveCycle {
                        side: PositionSide::LongDown,
                        entry_price,
                        size,
                        tp_price,
                        sl_price,
                    });

                    // Update fund used when position opens
                    self.total_fund_used += entry_price * size;

                    let msg = format!(
                        "[SIM] 🔴 BUY DOWN | asset={} | shares={} | entry={:.4} | TP={:.4} | SL={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, tp_price, sl_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    println!("{}", msg);
                    info!(
                        "[SIM] 🔴 OPEN CYCLE DOWN | asset={} | shares={} | entry={:.4} | TP={:.4} | SL={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, tp_price, sl_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    crate::log_trading_event(&msg);
                    // Simulate balance confirmation delay (5 seconds) before placing TP order
                    // In live mode, this delay happens automatically during balance confirmation
                    sleep(Duration::from_secs(5)).await;
                    
                    // TP: Place LIMIT SELL order for same token at TP price (matching live mode)
                    if tp_price <= Decimal::ONE {
                        let tp_price_rounded = tp_price.round_dp(2);
                        let limit_msg = format!(
                            "[SIM] 📌 LIMIT    | side=SELL | asset={} | token=DOWN | price={:.2} | shares={:.2}",
                            asset, tp_price_rounded, size
                        );
                        println!("{}", limit_msg);
                        crate::log_trading_event(&limit_msg);
                    } else {
                        let wait_msg = format!(
                            "[SIM] ⏸️  NO LIMIT | asset={} | TP={:.4} out of [0,1] | waiting for SL or market end",
                            asset, tp_price
                        );
                        println!("{}", wait_msg);
                        crate::log_trading_event(&wait_msg);
                    }
                }
                _ => {}
            }
        }

        // 3) Log index snapshot on every tick (whether or not we are in a cycle)
        if let Some(asset_name) = &price_point.asset {
            match (up_index, down_index) {
                (Some(ui), Some(di)) => {
                    let msg = match cfg.index_type {
                        IndexType::MACD => format!(
                            "[SIM] 📈 INDEX    | asset={} | {}_up={:.4} | {}_down={:.4} | pnl={:.4} | wins={} | losses={} | fund={:.4}",
                            asset_name, idx_name, ui, idx_name, di, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                        _ => format!(
                            "[SIM] 📈 INDEX    | asset={} | {}_up={:.2} | {}_down={:.2} | pnl={:.4} | wins={} | losses={} | fund={:.4}",
                            asset_name, idx_name, ui, idx_name, di, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                    };
                    println!("{}", msg);
                    match cfg.index_type {
                        IndexType::MACD => info!(
                            "[SIM] 📈 {} Up={:.4} Down={:.4} | asset={} | pnl={:.4} wins={} losses={} fund={:.4}",
                            idx_name, ui, di, asset_name, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                        _ => info!(
                            "[SIM] 📈 {} Up={:.2} Down={:.2} | asset={} | pnl={:.4} wins={} losses={} fund={:.4}",
                            idx_name, ui, di, asset_name, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                    };
                    crate::log_trading_event(&msg);
                }
                _ => {
                    let msg = format!(
                        "[SIM] 📈 INDEX    | asset={} | {}=n/a",
                        asset_name, idx_name
                    );
                    println!("{}", msg);
                    info!(
                        "[SIM] 📈 Index not ready ({}) for asset={}",
                        idx_name, asset_name
                    );
                    crate::log_trading_event(&msg);
                }
            }
        }

        Ok(())
    }

    /// Run simulation loop
    pub async fn run(&mut self) -> anyhow::Result<()> {
        println!("🎮 Simulation mode started");
        println!("   Strategy      : {}", self.strategy.name());
        println!("   Markets       : {:?}", self.trading_assets);
        println!("   Initial equity: ${:.2}", self.capital);
        println!("   Check interval: {} ms", self.config.get_check_interval_ms());
        info!("🎮 Starting SIMULATION MODE");
        info!("Strategy: {}", self.strategy.name());
        info!("Markets: {:?}", self.trading_assets);
        info!("Initial capital: ${:.2}", self.capital);
        info!("Check interval: {}ms", self.config.get_check_interval_ms());

        let check_interval = Duration::from_millis(self.config.get_check_interval_ms());

        loop {
            match self.monitor.fetch_market_data().await {
                Ok(snapshot) => {
                    if let Err(e) = self.process_snapshot(&snapshot).await {
                        warn!("Error processing snapshot: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Error fetching market data: {}", e);
                }
            }

            sleep(check_interval).await;
        }
    }
}
