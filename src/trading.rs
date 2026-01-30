// Real trading mode - monitoring and sending real orders

use crate::config::{CliConfig, StrategyConfig, IndexType};
use crate::monitor::{MarketMonitor, MarketSnapshot};
use crate::strategies::{Strategy, TradeAction, MomentumHedgeStrategy};
use crate::types::{PricePoint, TradingStats, ActiveCycle, PositionSide};
use crate::indicators::{RollingRSI, RollingMACD, RollingMomentum, calculate_rsi};
use crate::api::PolymarketApi;
use crate::models::OrderRequest;
use rust_decimal::Decimal;
use rust_decimal::prelude::{ToPrimitive, FromPrimitive};
use rust_decimal_macros::dec;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tracing::{info, warn, error};

// Polymarket conditional tokens use 6 decimals (10^6)
const TOKEN_DECIMALS: Decimal = dec!(1000000.0);

/// Format a long ID (token ID or order ID) to show only prefix and suffix for readability
fn format_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}...{}", &id[..6], &id[id.len()-6..])
    }
}

/// Format an optional ID
fn format_id_opt(id: &Option<String>) -> String {
    id.as_ref().map(|s| format_id(s)).unwrap_or_else(|| "None".to_string())
}

#[derive(Debug, Clone)]
struct PendingEntry {
    asset: String,
    side: PositionSide,
    token_id: String,
    limit_price: Decimal,
    requested_size: Decimal,
    pre_balance: Decimal,
    placed_at: Instant,
    entry_order_id: Option<String>,
}

/// Real trading mode - executes actual trades
pub struct LiveTrader {
    monitor: Arc<MarketMonitor>,
    api: Arc<PolymarketApi>,
    strategy: Box<dyn Strategy>,
    price_history: VecDeque<PricePoint>,
    stats: TradingStats,
    capital: Decimal,
    config: CliConfig,
    rsi_calculator: RollingRSI,
    macd_calculator: RollingMACD,
    momentum_calculator: RollingMomentum,
    trading_assets: Vec<String>,
    /// Current active trading cycle for the asset being traded
    current_cycle: Option<ActiveCycle>,
    /// Total PnL across trades for the current market (starts at 0 each new market)
    total_pnl: Decimal,
    /// Number of winning trades (TP or market-settlement win)
    wins: usize,
    /// Number of losing trades (SL or market-settlement loss)
    losses: usize,
    /// Total fund used (accumulates entry_price * size for each opened trade)
    total_fund_used: Decimal,
    /// Previous period timestamp to detect market rollover
    previous_period_timestamp: Option<u64>,
    /// Last price point per asset (used for market-end settlement if a cycle is still open)
    last_price_points: HashMap<String, PricePoint>,
    /// Pending entry order waiting to be filled (Approach A)
    pending_entry: Option<PendingEntry>,
    /// Track active order IDs for order management
    tp_order_id: Option<String>,
    sl_order_id: Option<String>,
    entry_order_id: Option<String>,
    /// Previous MACD value for Up token (for momentum acceleration check)
    previous_macd_up: Option<f64>,
    /// Previous MACD value for Down token (for momentum acceleration check)
    previous_macd_down: Option<f64>,
    /// Previous signal line value for Up token (for MACDSignal crossover detection)
    previous_signal_up: Option<f64>,
    /// Previous signal line value for Down token (for MACDSignal crossover detection)
    previous_signal_down: Option<f64>,
}

impl LiveTrader {
    pub fn new(
        monitor: Arc<MarketMonitor>,
        api: Arc<PolymarketApi>,
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

        // Apply enable_* flags from config.json
        trading_assets.retain(|asset| match asset.as_str() {
            "ETH" => config.is_eth_enabled(),
            "SOL" => config.is_solana_enabled(),
            "XRP" => config.is_xrp_enabled(),
            _ => true, // BTC always allowed
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
            api,
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
            last_price_points: HashMap::new(),
            pending_entry: None,
            tp_order_id: None,
            sl_order_id: None,
            entry_order_id: None,
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

    /// Reset indicators and price history for a new market
    fn reset_indicators_for_new_market(&mut self) {
        let cfg = self.strategy.config();
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
        self.price_history.clear();
        self.last_price_points.clear();
        self.pending_entry = None;
        // Reset previous MACD and signal line values when starting new market
        self.previous_macd_up = None;
        self.previous_macd_down = None;
        self.previous_signal_up = None;
        self.previous_signal_down = None;

        let msg = "[LIVE] 🔄 NEW MARKET | Resetting indicators and price history";
        println!("{}", msg);
        crate::log_trading_event(msg);
    }

    /// Reset per-market performance counters back to 0 (pnl/wins/losses/fund)
    fn reset_market_stats(&mut self) {
        self.total_pnl = Decimal::ZERO;
        self.wins = 0;
        self.losses = 0;
        self.total_fund_used = Decimal::ZERO;
        self.pending_entry = None;

        let msg = "[LIVE] 🔁 NEW MARKET | Resetting market stats (pnl/wins/losses/fund)";
        println!("{}", msg);
        crate::log_trading_event(msg);
    }

    /// Cancel any outstanding orders we are tracking (best-effort)
    async fn cancel_outstanding_orders(&mut self) {
        let ids = [
            ("ENTRY", self.entry_order_id.clone()),
            ("TP", self.tp_order_id.clone()),
            ("SL", self.sl_order_id.clone()),
        ];

        for (kind, id_opt) in ids {
            if let Some(id) = id_opt {
                match self.api.cancel_order(&id).await {
                    Ok(_) => {
                        let msg = format!("✅ [LIVE] Cancelled {} order: {}", kind, format_id(&id));
                        println!("{}", msg);
                        info!("{}", msg);
                        crate::log_trading_event(&msg);
                    }
                    Err(e) => {
                        let msg = format!("⚠️  [LIVE] Failed to cancel {} order {}: {}", kind, format_id(&id), e);
                        println!("{}", msg);
                        warn!("{}", msg);
                        crate::log_trading_event(&msg);
                    }
                }
            }
        }

        self.entry_order_id = None;
        self.tp_order_id = None;
        self.sl_order_id = None;
    }

    /// If we have a pending entry for this asset, try to confirm fill via balance delta.
    /// Returns Ok(true) if we handled a pending entry (filled or still waiting) and the caller should skip normal trading logic for this tick.
    async fn maybe_confirm_pending_entry(&mut self, asset: &str, cfg: &StrategyConfig, price_point: &PricePoint) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Min delta in smallest units: 0.001 tokens = 1000 smallest units (for 6 decimals)
        let min_delta = dec!(1000.0);
        let timeout_secs: u64 = 10;

        let pending = match &self.pending_entry {
            Some(p) if p.asset == asset => p.clone(),
            _ => return Ok(false),
        };

        // Timeout -> cancel and clear pending
        if pending.placed_at.elapsed().as_secs() >= timeout_secs {
            if let Some(id) = &pending.entry_order_id {
                let msg = format!("⏳ [LIVE] ENTRY TIMEOUT | asset={} | order_id={} | cancelling entry", asset, format_id(id));
                println!("{}", msg);
                crate::log_trading_event(&msg);
                let _ = self.api.cancel_order(id).await;
            }
            self.pending_entry = None;
            self.entry_order_id = None;
            return Ok(false);
        }

        // Check balance
        let current_balance = match self.api.check_balance_only(&pending.token_id).await {
            Ok(b) => b,
            Err(e) => {
                let msg = format!(
                    "⚠️  [LIVE] ENTRY PENDING | asset={} | token={} | balance check failed: {} (will retry)",
                    asset, format_id(&pending.token_id), e
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                return Ok(true);
            }
        };

        // If balance decreased (e.g., from a previous TP order that just filled), update pre_balance
        // This ensures we calculate filled_size correctly based on the actual current balance
        let effective_pre_balance = if current_balance < pending.pre_balance {
            // Balance decreased - update pre_balance to current balance
            if let Some(p) = &mut self.pending_entry {
                if p.asset == asset {
                    let old_pre_balance_normalized = pending.pre_balance / TOKEN_DECIMALS;
                    let new_pre_balance_normalized = current_balance / TOKEN_DECIMALS;
                    let msg = format!(
                        "🔄 [LIVE] BALANCE DECREASED | asset={} | updating pre_balance from {:.6} to {:.6} (likely from TP sell)",
                        asset, old_pre_balance_normalized, new_pre_balance_normalized
                    );
                    println!("{}", msg);
                    crate::log_trading_event(&msg);
                    p.pre_balance = current_balance;
                }
            }
            current_balance // Use current balance as the new baseline
        } else {
            pending.pre_balance // Use original pre_balance
        };

        if current_balance > effective_pre_balance + min_delta {
            // Balance is in smallest unit (6 decimals), normalize to actual token amount
            let filled_size_raw = current_balance - effective_pre_balance;
            let filled_size = filled_size_raw / TOKEN_DECIMALS;

            // Cancel any remaining unfilled entry
            if let Some(id) = &pending.entry_order_id {
                let msg = format!(
                    "✅ [LIVE] ENTRY FILLED | asset={} | order_id={} | filled_size={:.6} | cancelling remaining entry",
                    asset, format_id(id), filled_size
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                let _ = self.api.cancel_order(id).await;
            }

            // Wait a bit and confirm balance is stable before placing TP/SL orders
            // This ensures the balance is fully confirmed/settled
            sleep(Duration::from_millis(500)).await;
            
            // Re-check balance to confirm it's stable
            let confirmed_balance = match self.api.check_balance_only(&pending.token_id).await {
                Ok(b) => b,
                Err(e) => {
                    let msg = format!(
                        "⚠️  [LIVE] Balance confirmation failed for {}: {} | retrying in next tick",
                        asset, e
                    );
                    println!("{}", msg);
                    crate::log_trading_event(&msg);
                    return Ok(true); // Retry in next tick
                }
            };

            // Verify balance is still at least as high as detected fill
            // Use the effective pre_balance (which may have been updated)
            let current_effective_pre_balance = if let Some(p) = &self.pending_entry {
                if p.asset == asset { p.pre_balance } else { effective_pre_balance }
            } else {
                effective_pre_balance
            };
            
            if confirmed_balance < current_effective_pre_balance + min_delta {
                // Normalize for display
                let current_balance_normalized = current_balance / TOKEN_DECIMALS;
                let confirmed_balance_normalized = confirmed_balance / TOKEN_DECIMALS;
                let msg = format!(
                    "⚠️  [LIVE] Balance decreased after fill detection | asset={} | initial={:.6} | confirmed={:.6} | retrying",
                    asset, current_balance_normalized, confirmed_balance_normalized
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
                return Ok(true); // Retry in next tick
            }

            // Use confirmed balance for filled_size (normalize from smallest unit)
            // Use the effective pre_balance (which may have been updated if balance decreased)
            let final_effective_pre_balance = if let Some(p) = &self.pending_entry {
                if p.asset == asset { p.pre_balance } else { effective_pre_balance }
            } else {
                effective_pre_balance
            };
            
            let confirmed_filled_size_raw = confirmed_balance - final_effective_pre_balance;
            let confirmed_filled_size = confirmed_filled_size_raw / TOKEN_DECIMALS;
            let msg = format!(
                "✅ [LIVE] BALANCE CONFIRMED | asset={} | filled_size={:.6} | placing TP order",
                asset, confirmed_filled_size
            );
            println!("{}", msg);
            crate::log_trading_event(&msg);

            // Now that we have a confirmed filled size, compute TP/SL from entry limit price
            let entry_price = pending.limit_price;
            let tp_price = entry_price + cfg.profit_threshold;
            let sl_price = entry_price - cfg.sl_threshold;

            // Place TP order first (SL will be checked after)
            self.tp_order_id = None;
            self.sl_order_id = None;

            if tp_price <= Decimal::ONE {
                // TP: Place LIMIT SELL order for same token at TP price
                let tp_price_rounded = tp_price.round_dp(2);
                let tp_order = OrderRequest {
                    token_id: pending.token_id.clone(),
                    side: "SELL".to_string(),
                    size: format!("{:.2}", confirmed_filled_size),
                    price: format!("{:.2}", tp_price_rounded),
                    order_type: "LIMIT".to_string(),
                };
                match self.api.place_order(&tp_order).await {
                    Ok(resp) => {
                        self.tp_order_id = resp.order_id.clone();
                        let msg = format!(
                            "✅ [LIVE] TP ORDER (post-fill) | asset={} | order_id={} | side=SELL | token={} | price={:.2} | size={:.2}",
                            asset, format_id_opt(&resp.order_id), format_id(&pending.token_id), tp_price_rounded, confirmed_filled_size
                        );
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                    }
                    Err(e) => {
                        let msg = format!("❌ [LIVE] Failed to place TP order (post-fill): {}", e);
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                    }
                }
            } else {
                let msg = format!(
                    "⏸️  [LIVE] NO TP | asset={} | tp_price={:.4} out of [0,1] | waiting for SL or market end",
                    asset, tp_price
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
            }

            // Check if SL condition is met during balance confirmation (after TP order is placed)
            // SL: Check opposite token ask price (SL = buy opposite token at (1 - SL))
            let opposite_token_price_f64 = match pending.side {
                PositionSide::LongUp => price_point.down_price,  // We bought Up, check Down ask price
                PositionSide::LongDown => price_point.up_price,   // We bought Down, check Up ask price
                PositionSide::Flat => 0.0,
            };
            
            let opposite_sl_price = Decimal::ONE - sl_price;
            let sl_hit_during_confirmation = if opposite_token_price_f64 > 0.0 {
                if let Some(opposite_token_ask_price) = Decimal::from_f64(opposite_token_price_f64) {
                    // SL hit: opposite token ask price is at or above (1 - SL)
                    opposite_token_ask_price >= opposite_sl_price
                } else {
                    false
                }
            } else {
                false
            };

            // If SL is hit during balance confirmation, cancel TP order and place SL market order
            // For MACD mode: also check if MACD of held token is <= 0
            let should_trigger_sl_confirmation = if sl_hit_during_confirmation {
                if cfg.index_type == IndexType::MACD && cfg.use_macd_sl_filter {
                    // Get MACD value of the token we're holding
                    let held_token_macd = match pending.side {
                        PositionSide::LongUp => {
                            // We're holding Up token - use the main MACD calculator
                            self.macd_calculator.get_macd()
                        }
                        PositionSide::LongDown => {
                            // We're holding Down token - need to calculate from price history
                            // Build temporary MACD calculator for Down token
                            let mut temp_macd_down = RollingMACD::new(cfg.macd_fast_period, cfg.macd_slow_period);
                            for p in &self.price_history {
                                temp_macd_down.add_price(p.down_price);
                            }
                            temp_macd_down.get_macd()
                        }
                        PositionSide::Flat => None,
                    };
                    
                    match held_token_macd {
                        Some(macd_value) => {
                            if macd_value > 0.0 {
                                // MACD still positive - don't trigger SL
                                // Only log if price condition was actually met
                                if sl_hit_during_confirmation {
                                    let msg = format!(
                                        "⏸️  [LIVE] SL SKIPPED (MACD > 0 during confirmation) | asset={} | side={:?} | MACD={:.4} > 0 | price condition met but momentum still positive",
                                        asset, pending.side, macd_value
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
                            // MACD not available - proceed with SL (fallback)
                            true
                        }
                    }
                } else {
                    // Not MACD mode or filter disabled - use price-based SL only
                    true
                }
            } else {
                false
            };
            
            if should_trigger_sl_confirmation {
                let msg = format!(
                    "⚠️  [LIVE] SL HIT DURING BALANCE CONFIRMATION | asset={} | side={:?} | entry={:.4} | sl={:.4} | opposite_ask={:.4} | target=(1-SL)={:.4}",
                    asset, pending.side, entry_price, sl_price, opposite_token_price_f64, opposite_sl_price
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);

                // Place MARKET order for opposite token to execute stop loss immediately
                let opposite_token_id = match pending.side {
                    PositionSide::LongUp => {
                        match self.monitor.get_down_token_id(asset).await {
                            Ok(id) => Some(id),
                            Err(e) => {
                                let msg = format!("❌ [LIVE] Failed to get Down token ID for SL execution: {}", e);
                                println!("{}", msg);
                                crate::log_trading_event(&msg);
                                None
                            }
                        }
                    }
                    PositionSide::LongDown => {
                        match self.monitor.get_up_token_id(asset).await {
                            Ok(id) => Some(id),
                            Err(e) => {
                                let msg = format!("❌ [LIVE] Failed to get Up token ID for SL execution: {}", e);
                                println!("{}", msg);
                                crate::log_trading_event(&msg);
                                None
                            }
                        }
                    }
                    PositionSide::Flat => None,
                };

                if let Some(opposite_token_id) = opposite_token_id {
                    // Place limit order at current ask price to execute immediately (market-like execution)
                    // Use current ask price rounded to 2 decimals to match tick size
                    let market_price = if let Some(ask_price) = Decimal::from_f64(opposite_token_price_f64) {
                        ask_price.round_dp(2)
                    } else {
                        opposite_sl_price.round_dp(2) // Fallback to (1-SL) if conversion fails
                    };
                    
                    let sl_order = OrderRequest {
                        token_id: opposite_token_id.clone(),
                        side: "BUY".to_string(),
                        size: format!("{:.2}", confirmed_filled_size),
                        price: format!("{:.2}", market_price),
                        order_type: "LIMIT".to_string(), // Use LIMIT with market price for immediate execution
                    };

                    match self.api.place_order(&sl_order).await {
                        Ok(resp) => {
                            let msg = format!(
                                "✅ [LIVE] SL MARKET ORDER PLACED | asset={} | order_id={} | side=BUY | opposite_token={} | size={:.2}",
                                asset, format_id_opt(&resp.order_id), format_id(&opposite_token_id), confirmed_filled_size
                            );
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                        }
                        Err(e) => {
                            let msg = format!("❌ [LIVE] Failed to place SL market order: {}", e);
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                        }
                    }
                }

                // Calculate PnL for this cycle
                let pnl = (sl_price - entry_price) * confirmed_filled_size;
                self.total_pnl += pnl;
                self.losses += 1;
                self.total_fund_used += entry_price * confirmed_filled_size;

                let msg = format!(
                    "❌ [LIVE] SL EXECUTED | asset={} | side={:?} | entry={:.4} | sl={:.4} | size={:.4} | pnl={:.4}",
                    asset, pending.side, entry_price, sl_price, confirmed_filled_size, pnl
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);

                // Cancel TP order if it was placed
                if let Some(tp_id) = &self.tp_order_id {
                    match self.api.cancel_order(tp_id).await {
                        Ok(_) => {
                            let msg = format!("✅ [LIVE] Cancelled TP order (SL hit during confirmation): {}", format_id(tp_id));
                            println!("{}", msg);
                            info!("{}", msg);
                            crate::log_trading_event(&msg);
                        }
                        Err(e) => {
                            let msg = format!("⚠️  [LIVE] Failed to cancel TP order {}: {}", format_id(tp_id), e);
                            println!("{}", msg);
                            warn!("{}", msg);
                            crate::log_trading_event(&msg);
                        }
                    }
                }

                // Clear all order IDs and close cycle
                self.tp_order_id = None;
                self.sl_order_id = None;
                self.pending_entry = None;
                self.entry_order_id = None;
                self.current_cycle = None;

                return Ok(true);
            }

            // Note: SL order is NOT placed upfront. It will be placed when price monitoring detects SL hit.
            // This is because placing a BUY limit order at (1-SL) would execute immediately if current price is below that.

            // Open cycle with confirmed filled size
            self.current_cycle = Some(ActiveCycle {
                side: pending.side.clone(),
                entry_price,
                size: confirmed_filled_size,
                tp_price,
                sl_price,
            });
            self.total_fund_used += entry_price * confirmed_filled_size;

            // Clear pending + entry id
            self.pending_entry = None;
            self.entry_order_id = None;

            return Ok(true);
        }

        // Still waiting - normalize balances for display
        // Use effective_pre_balance (which may have been updated if balance decreased)
        let display_pre_balance = if let Some(p) = &self.pending_entry {
            if p.asset == asset { p.pre_balance } else { effective_pre_balance }
        } else {
            effective_pre_balance
        };
        let pre_balance_normalized = display_pre_balance / TOKEN_DECIMALS;
        let current_balance_normalized = current_balance / TOKEN_DECIMALS;
            let msg = format!(
                "⏳ [LIVE] ENTRY PENDING | asset={} | token={} | pre_balance={:.6} | current_balance={:.6}",
                asset, format_id(&pending.token_id), pre_balance_normalized, current_balance_normalized
            );
        println!("{}", msg);
        crate::log_trading_event(&msg);
        Ok(true)
    }

    /// Handle market end (period rollover): settle any open cycle using final 0/1 outcome prices and print summary.
    async fn handle_market_end(&mut self, asset: &str) {
        // If we have a pending entry for this asset, cancel it and clear state
        if let Some(p) = &self.pending_entry {
            if p.asset == asset {
                if let Some(id) = &p.entry_order_id {
                    let msg = format!("🧹 [LIVE] MARKET END | asset={} | cancelling pending entry order {}", asset, format_id(id));
                    println!("{}", msg);
                    crate::log_trading_event(&msg);
                    let _ = self.api.cancel_order(id).await;
                }
                self.pending_entry = None;
                self.entry_order_id = None;
            }
        }

        if let Some(cycle) = &self.current_cycle {
            if let Some(pp) = self.last_price_points.get(asset) {
                let market_outcome_up = pp.up_price >= 0.99;
                let market_outcome_down = pp.down_price >= 0.99;

                let (final_pnl, is_win, outcome_str) = match cycle.side {
                    PositionSide::LongUp => {
                        if market_outcome_up {
                            ((Decimal::ONE - cycle.entry_price) * cycle.size, true, "UP")
                        } else {
                            ((Decimal::ZERO - cycle.entry_price) * cycle.size, false, "DOWN")
                        }
                    }
                    PositionSide::LongDown => {
                        if market_outcome_down {
                            ((Decimal::ONE - cycle.entry_price) * cycle.size, true, "DOWN")
                        } else {
                            ((Decimal::ZERO - cycle.entry_price) * cycle.size, false, "UP")
                        }
                    }
                    PositionSide::Flat => (Decimal::ZERO, false, "UNKNOWN"),
                };

                self.total_pnl += final_pnl;
                if is_win {
                    self.wins += 1;
                } else {
                    self.losses += 1;
                }

                let msg = format!(
                    "[LIVE] 🏁 MARKET END | asset={} | side={:?} | entry={:.4} | outcome={} | pnl={:.4} | {}",
                    asset,
                    cycle.side,
                    cycle.entry_price,
                    outcome_str,
                    final_pnl,
                    if is_win { "WIN" } else { "LOSS" }
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
            } else {
                let msg = format!(
                    "⚠️  [LIVE] MARKET END | asset={} | open cycle exists but no last price point stored; cannot settle PnL",
                    asset
                );
                println!("{}", msg);
                crate::log_trading_event(&msg);
            }
        }

        // Always cancel outstanding orders on rollover (best-effort) and close cycle locally.
        self.cancel_outstanding_orders().await;
        self.current_cycle = None;

        let summary_msg = format!(
            "[LIVE] 📊 MARKET SUMMARY | asset={} | total_pnl={:.4} | wins={} | losses={} | fund_used={:.4}",
            asset, self.total_pnl, self.wins, self.losses, self.total_fund_used
        );
        println!("{}", summary_msg);
        crate::log_trading_event(&summary_msg);
    }

    /// Process a snapshot and make trading decisions
    async fn process_snapshot(&mut self, snapshot: &MarketSnapshot) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Detect market rollover (new 15-min period)
        let current_period = snapshot.period_timestamp;
        if let Some(prev_period) = self.previous_period_timestamp {
            if prev_period != current_period {
                // Market ended - FIRST log a separator, then handle final PnL for each asset
                let separator_msg = "[LIVE] ════════════════════════════════════════════════════════════════════════════";
                println!("{}", separator_msg);
                crate::log_trading_event(separator_msg);
                
                let assets = self.trading_assets.clone();
                for asset in &assets {
                    self.handle_market_end(asset).await;
                }
                
                // Log another separator after all summaries
                let separator_end = "[LIVE] ═════════════════════════════════════════════════════════════════════════════";
                println!("{}", separator_end);
                crate::log_trading_event(separator_end);
                
                self.reset_indicators_for_new_market();
                self.reset_market_stats();
                // Ensure pending entry is cleared on rollover
                self.pending_entry = None;
            }
        }
        self.previous_period_timestamp = Some(current_period);

        let assets = self.trading_assets.clone();
        for asset in &assets {
            if let Some(price_point) = Self::snapshot_to_price_point(snapshot, asset) {
                // Track last price point (for market-end settlement)
                self.last_price_points.insert(asset.clone(), price_point.clone());
                self.process_price_point(&price_point).await?;
            }
        }
        Ok(())
    }

    /// Process a single price point and execute trades
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
                        let start = prices.len().saturating_sub(cfg.lookback + 1);
                        let slice: Vec<f64> = prices[start..].iter().map(|p| p.down_price).collect();
                        if slice.len() >= cfg.lookback + 1 {
                            calculate_rsi(&slice, cfg.lookback)
                        } else {
                            None
                        }
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
                    "[LIVE] 🔀 MACD CROSSOVER | asset={} | token=UP | macd={:.4} | signal={:.4} | price={:.4}",
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
                    "[LIVE] 🔀 MACD CROSSOVER | asset={} | token=DOWN | macd={:.4} | signal={:.4} | price={:.4}",
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

        let idx_name = match cfg.index_type {
            IndexType::RSI => "RSI",
            IndexType::MACD => "MACD",
            IndexType::MACDSignal => "MACD_SIG",
            IndexType::Momentum => "MOM",
        };

        let asset = price_point
            .asset
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string());

        // If we have a pending entry for this asset, prioritize confirming fill (Approach A)
        // and skip normal trading logic until the pending entry is resolved.
        if self.current_cycle.is_none() {
            let cfg_tmp = self.strategy.config().clone();
            if self.maybe_confirm_pending_entry(&asset, &cfg_tmp, price_point).await? {
                return Ok(());
            }
        }

        // 1) If a cycle is already open, check TP/SL based on latest price
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
                // Take‑profit hit: same token ask price reaches TP
                if let Some(tp_price) = Decimal::from_f64(same_token_price_f64) {
                    if cycle.tp_price <= Decimal::ONE && tp_price >= cycle.tp_price {
                        let pnl = (cycle.tp_price - cycle.entry_price) * cycle.size;
                        // Update per-market stats (fund is counted when position opens)
                        self.total_pnl += pnl;
                        self.wins += 1;
                        let msg = format!(
                            "✅ [LIVE] TP HIT   | asset={} | side={:?} | entry={:.4} | tp={:.4} | size={:.4} | pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.tp_price,
                            cycle.size,
                            pnl
                        );
                        println!("{}", msg);
                        info!(
                            "[LIVE] TP HIT | asset={} side={:?} entry={:.4} tp={:.4} size={:.4} pnl={:.4}",
                            asset,
                            cycle.side,
                            cycle.entry_price,
                            cycle.tp_price,
                            cycle.size,
                            pnl
                        );
                        crate::log_trading_event(&msg);
                        
                        // Cancel SL order since TP was hit
                        if let Some(sl_id) = &self.sl_order_id {
                            match self.api.cancel_order(sl_id).await {
                                Ok(_) => {
                                    info!("✅ Cancelled SL order: {}", format_id(sl_id));
                                    crate::log_trading_event(&format!("✅ Cancelled SL order: {}", format_id(sl_id)));
                                }
                                Err(e) => {
                                    warn!("⚠️  Failed to cancel SL order {}: {}", format_id(sl_id), e);
                                    crate::log_trading_event(&format!("⚠️  Failed to cancel SL order {}: {}", format_id(sl_id), e));
                                }
                            }
                        }
                        
                        // Clear all order IDs and close cycle
                        self.sl_order_id = None;
                        self.tp_order_id = None;
                        self.entry_order_id = None;
                        self.current_cycle = None;
                    }
                }
            }
            
            // Stop-loss hit: check if opposite token ask price is at or above (1 - SL)
            // When SL is hit, we buy opposite token at (1 - SL) to stop loss
            // Note: When same token price drops, opposite token price rises, so condition is reversed (>= instead of <=)
            if opposite_token_price_f64 > 0.0 {
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
                                            "⏸️  [LIVE] SL SKIPPED (MACD > 0) | asset={} | side={:?} | MACD={:.4} > 0 | price condition met but momentum still positive",
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
                        // Place BUY order for opposite token at (1 - SL) to execute stop loss
                        let opposite_sl_price = Decimal::ONE - cycle.sl_price;
                        let opposite_sl_price_rounded = opposite_sl_price.round_dp(2);
                        
                        // Get opposite token ID
                        let opposite_token_id = match cycle.side {
                            PositionSide::LongUp => {
                                // We bought Up, so SL is BUY Down at (1 - SL)
                                match self.monitor.get_down_token_id(&asset).await {
                                    Ok(id) => Some(id),
                                    Err(e) => {
                                        let msg = format!("❌ [LIVE] Failed to get Down token ID for SL execution: {}", e);
                                        println!("{}", msg);
                                        crate::log_trading_event(&msg);
                                        None
                                    }
                                }
                            }
                            PositionSide::LongDown => {
                                // We bought Down, so SL is BUY Up at (1 - SL)
                                match self.monitor.get_up_token_id(&asset).await {
                                    Ok(id) => Some(id),
                                    Err(e) => {
                                        let msg = format!("❌ [LIVE] Failed to get Up token ID for SL execution: {}", e);
                                        println!("{}", msg);
                                        crate::log_trading_event(&msg);
                                        None
                                    }
                                }
                            }
                            PositionSide::Flat => None,
                        };
                        
                        // Place BUY order for opposite token at (1 - SL) to stop loss
                        if let Some(opposite_token_id) = opposite_token_id {
                            let sl_order = OrderRequest {
                                token_id: opposite_token_id.clone(),
                                side: "BUY".to_string(),
                                size: format!("{:.2}", cycle.size),
                                price: format!("{:.2}", opposite_sl_price_rounded),
                                order_type: "LIMIT".to_string(),
                            };
                            
                            match self.api.place_order(&sl_order).await {
                                Ok(resp) => {
                                    self.sl_order_id = resp.order_id.clone();
                                    let msg = format!(
                                        "✅ [LIVE] SL ORDER PLACED | asset={} | order_id={} | side=BUY | opposite_token={} | price={:.2} (1-SL={:.2}) | size={:.2}",
                                        asset, format_id_opt(&resp.order_id), format_id(&opposite_token_id), opposite_sl_price_rounded, cycle.sl_price, cycle.size
                                    );
                                    println!("{}", msg);
                                    crate::log_trading_event(&msg);
                                }
                                Err(e) => {
                                    let msg = format!("❌ [LIVE] Failed to place SL order on hit: {}", e);
                                    println!("{}", msg);
                                    crate::log_trading_event(&msg);
                                }
                            }
                        }
                        
                        let pnl = (cycle.sl_price - cycle.entry_price) * cycle.size;
                        // Update per-market stats (fund is counted when position opens)
                        self.total_pnl += pnl;
                        self.losses += 1;
                        let msg = format!(
                            "❌ [LIVE] SL HIT   | asset={} | side={:?} | entry={:.4} | sl={:.4} | opposite_ask={:.4} | target=(1-SL)={:.4} | size={:.4} | pnl={:.4}",
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
                            "[LIVE] SL HIT | asset={} side={:?} entry={:.4} sl={:.4} opposite_ask={:.4} target=(1-SL)={:.4} size={:.4} pnl={:.4}",
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
                        
                        // Cancel TP order since SL was hit
                        if let Some(tp_id) = &self.tp_order_id {
                            match self.api.cancel_order(tp_id).await {
                                Ok(_) => {
                                    info!("✅ Cancelled TP order: {}", format_id(tp_id));
                                    crate::log_trading_event(&format!("✅ Cancelled TP order: {}", format_id(tp_id)));
                                }
                                Err(e) => {
                                    warn!("⚠️  Failed to cancel TP order {}: {}", format_id(tp_id), e);
                                    crate::log_trading_event(&format!("⚠️  Failed to cancel TP order {}: {}", format_id(tp_id), e));
                                }
                            }
                        }
                        
                        // Clear all order IDs and close cycle
                        self.tp_order_id = None;
                        self.entry_order_id = None;
                        self.current_cycle = None;
                    }
                }
            }
        }

        // 2) If flat and strategy says BUY, open new cycle (and in future, send real orders)
        if self.current_cycle.is_none() && self.pending_entry.is_none() {
            // Helper: format Option<f64> indices - 4 decimals for MACD and MACDSignal, 2 decimals for others
            let up_idx_str = match (up_index, cfg.index_type) {
                (Some(v), IndexType::MACD) | (Some(v), IndexType::MACDSignal) => format!("{:.4}", v),
                (Some(v), _) => format!("{:.2}", v),
                (None, _) => "n/a".to_string(),
            };
            let down_idx_str = match (down_index, cfg.index_type) {
                (Some(v), IndexType::MACD) | (Some(v), IndexType::MACDSignal) => format!("{:.4}", v),
                (Some(v), _) => format!("{:.2}", v),
                (None, _) => "n/a".to_string(),
            };

            match &action {
                TradeAction::BuyUp { price, shares } => {
                    let entry_price = *price;
                    let size = *shares;
                    
                    // For MACD mode: Check if MACD is increasing (momentum acceleration)
                    if cfg.index_type == IndexType::MACD && !macd_increasing_check.0 {
                        let msg = format!(
                            "⏸️  [LIVE] MACD NOT INCREASING | asset={} | MACD_up={:.4} | previous={:.4} | momentum not accelerating",
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
                                "⏸️  [LIVE] TRADING NOT STARTED | asset={} | remaining={}m > {}m | waiting for market to reach {}m remaining",
                                asset, remaining_minutes, required_remaining_minutes, required_remaining_minutes
                            );
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                            return Ok(()); // Skip placing entry order
                        }
                    }
                    
                    // Check if we should skip placing entry order: if entry price > 0.93 and time < 13 minutes
                    let current_time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let period_start = price_point.timestamp;
                    let elapsed_seconds = current_time.saturating_sub(period_start);
                    let thirteen_minutes = 13 * 60; // 780 seconds
                    
                    let entry_price_f64 = entry_price.to_f64().unwrap_or(0.0);
                    let should_skip = entry_price_f64 > 0.93 && elapsed_seconds < thirteen_minutes;
                    
                    if should_skip {
                        let msg = format!(
                            "⏸️  [LIVE] ENTRY SKIPPED | asset={} | entry={:.4} > 0.93 | elapsed={}s < 13m | not placing entry order",
                            asset, entry_price, elapsed_seconds
                        );
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                        return Ok(()); // Skip placing entry order
                    }
                    
                    let msg = format!(
                        "🟢 [LIVE] SIGNAL BUY UP | asset={} | shares={} | entry_limit={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    println!("{}", msg);
                    info!("{}", msg);
                    crate::log_trading_event(&msg);
                    
                    // Get Up token ID for placing the ENTRY order
                    match self.monitor.get_up_token_id(&asset).await {
                        Ok(up_token_id) => {
                            // Wait a bit to ensure any previous TP/SL orders have settled
                            // This ensures the balance reflects the actual current state
                            sleep(Duration::from_millis(500)).await;
                            
                            // Record pre-balance before placing entry (so we can confirm fill)
                            // Check balance multiple times to ensure it's settled
                            let pre_balance = match self.api.check_balance_only(&up_token_id).await {
                                Ok(b) => {
                                    // Wait and check again to ensure balance is stable
                                    sleep(Duration::from_millis(500)).await;
                                    match self.api.check_balance_only(&up_token_id).await {
                                        Ok(b2) => {
                                            // Use the second check as it's more likely to be settled
                                            if (b2 - b).abs() < dec!(1000.0) {
                                                b2 // Balance is stable
                                            } else {
                                                b // Use first check if balance changed significantly
                                            }
                                        }
                                        Err(_) => b, // Fallback to first check
                                    }
                                }
                                Err(e) => {
                                    let err_msg = format!("❌ [LIVE] Failed to check pre-balance for entry: {}", e);
                                    println!("{}", err_msg);
                                    crate::log_trading_event(&err_msg);
                                    return Ok(());
                                }
                            };

                            // Place ENTRY buy order (buy Up tokens at entry_limit)
                            // Round price to 2 decimal places (Polymarket minimum tick size is 0.01)
                            let entry_price_rounded = entry_price.round_dp(2);
                            let entry_order = OrderRequest {
                                token_id: up_token_id.clone(),
                                side: "BUY".to_string(),
                                size: format!("{:.2}", size),
                                price: format!("{:.2}", entry_price_rounded),
                                order_type: "LIMIT".to_string(),
                            };
                            
                            match self.api.place_order(&entry_order).await {
                                Ok(resp) => {
                                    self.entry_order_id = resp.order_id.clone();
                                    self.pending_entry = Some(PendingEntry {
                                        asset: asset.clone(),
                                        side: PositionSide::LongUp,
                                        token_id: up_token_id.clone(),
                                        limit_price: entry_price,
                                        requested_size: size,
                                        pre_balance,
                                        placed_at: Instant::now(),
                                        entry_order_id: resp.order_id.clone(),
                                    });
                                    let order_msg = format!(
                                        "✅ [LIVE] ENTRY ORDER PLACED | asset={} | order_id={} | token={} | price={:.2} | size={:.2} | pre_balance={:.6}",
                                        asset, format_id_opt(&resp.order_id), format_id(&up_token_id), entry_price_rounded, size, pre_balance
                                    );
                                    println!("{}", order_msg);
                                    info!("{}", order_msg);
                                    crate::log_trading_event(&order_msg);
                                }
                                Err(e) => {
                                    let err_msg = format!("❌ [LIVE] Failed to place entry order: {}", e);
                                    error!("{}", err_msg);
                                    crate::log_trading_event(&err_msg);
                                }
                            }
                        }
                        Err(e) => {
                            let err_msg = format!("❌ [LIVE] Failed to get Up token ID for {}: {}", asset, e);
                            error!("{}", err_msg);
                            crate::log_trading_event(&err_msg);
                        }
                    }
                }
                TradeAction::BuyDown { price, shares } => {
                    let entry_price = *price;
                    let size = *shares;
                    
                    // For MACD mode: Check if MACD is increasing (momentum acceleration)
                    if cfg.index_type == IndexType::MACD && !macd_increasing_check.1 {
                        let msg = format!(
                            "⏸️  [LIVE] MACD NOT INCREASING | asset={} | MACD_down={:.4} | previous={:.4} | momentum not accelerating",
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
                                "⏸️  [LIVE] TRADING NOT STARTED | asset={} | remaining={}m > {}m | waiting for market to reach {}m remaining",
                                asset, remaining_minutes, required_remaining_minutes, required_remaining_minutes
                            );
                            println!("{}", msg);
                            crate::log_trading_event(&msg);
                            return Ok(()); // Skip placing entry order
                        }
                    }
                    
                    // Check if we should skip placing entry order: if entry price > 0.93 and time < 13 minutes
                    let current_time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let period_start = price_point.timestamp;
                    let elapsed_seconds = current_time.saturating_sub(period_start);
                    let thirteen_minutes = 13 * 60; // 780 seconds
                    
                    let entry_price_f64 = entry_price.to_f64().unwrap_or(0.0);
                    let should_skip = entry_price_f64 > 0.93 && elapsed_seconds < thirteen_minutes;
                    
                    if should_skip {
                        let msg = format!(
                            "⏸️  [LIVE] ENTRY SKIPPED | asset={} | entry={:.4} > 0.93 | elapsed={}s < 13m | not placing entry order",
                            asset, entry_price, elapsed_seconds
                        );
                        println!("{}", msg);
                        crate::log_trading_event(&msg);
                        return Ok(()); // Skip placing entry order
                    }
                    
                    let msg = format!(
                        "🔴 [LIVE] SIGNAL BUY DOWN | asset={} | shares={} | entry_limit={:.4} | {}_up={} | {}_down={}",
                        asset, size, entry_price, idx_name, up_idx_str, idx_name, down_idx_str
                    );
                    println!("{}", msg);
                    info!("{}", msg);
                    crate::log_trading_event(&msg);
                    
                    // Get Down token ID for placing the ENTRY order
                    match self.monitor.get_down_token_id(&asset).await {
                        Ok(down_token_id) => {
                            // Wait a bit to ensure any previous TP/SL orders have settled
                            // This ensures the balance reflects the actual current state
                            sleep(Duration::from_millis(500)).await;
                            
                            // Record pre-balance before placing entry (so we can confirm fill)
                            // Check balance multiple times to ensure it's settled
                            let pre_balance = match self.api.check_balance_only(&down_token_id).await {
                                Ok(b) => {
                                    // Wait and check again to ensure balance is stable
                                    sleep(Duration::from_millis(500)).await;
                                    match self.api.check_balance_only(&down_token_id).await {
                                        Ok(b2) => {
                                            // Use the second check as it's more likely to be settled
                                            if (b2 - b).abs() < dec!(1000.0) {
                                                b2 // Balance is stable
                                            } else {
                                                b // Use first check if balance changed significantly
                                            }
                                        }
                                        Err(_) => b, // Fallback to first check
                                    }
                                }
                                Err(e) => {
                                    let err_msg = format!("❌ [LIVE] Failed to check pre-balance for entry: {}", e);
                                    println!("{}", err_msg);
                                    crate::log_trading_event(&err_msg);
                                    return Ok(());
                                }
                            };

                            // Place ENTRY buy order (buy Down tokens at entry_limit)
                            // Round price to 2 decimal places (Polymarket minimum tick size is 0.01)
                            let entry_price_rounded = entry_price.round_dp(2);
                            let entry_order = OrderRequest {
                                token_id: down_token_id.clone(),
                                side: "BUY".to_string(),
                                size: format!("{:.2}", size),
                                price: format!("{:.2}", entry_price_rounded),
                                order_type: "LIMIT".to_string(),
                            };
                            
                            match self.api.place_order(&entry_order).await {
                                Ok(resp) => {
                                    self.entry_order_id = resp.order_id.clone();
                                    self.pending_entry = Some(PendingEntry {
                                        asset: asset.clone(),
                                        side: PositionSide::LongDown,
                                        token_id: down_token_id.clone(),
                                        limit_price: entry_price,
                                        requested_size: size,
                                        pre_balance,
                                        placed_at: Instant::now(),
                                        entry_order_id: resp.order_id.clone(),
                                    });
                                    let order_msg = format!(
                                        "✅ [LIVE] ENTRY ORDER PLACED | asset={} | order_id={} | token={} | price={:.2} | size={:.2} | pre_balance={:.6}",
                                        asset, format_id_opt(&resp.order_id), format_id(&down_token_id), entry_price_rounded, size, pre_balance
                                    );
                                    println!("{}", order_msg);
                                    info!("{}", order_msg);
                                    crate::log_trading_event(&order_msg);
                                }
                                Err(e) => {
                                    let err_msg = format!("❌ [LIVE] Failed to place entry order: {}", e);
                                    error!("{}", err_msg);
                                    crate::log_trading_event(&err_msg);
                                }
                            }
                        }
                        Err(e) => {
                            let err_msg = format!("❌ [LIVE] Failed to get Down token ID for {}: {}", asset, e);
                            error!("{}", err_msg);
                            crate::log_trading_event(&err_msg);
                        }
                    }
                }
                _ => {}
            }
        }

        // 3) Log snapshot of price + trending indices + trading stats (for monitoring)
        if let Some(asset_name) = &price_point.asset {
            match (up_index, down_index) {
                (Some(ui), Some(di)) => {
                    let msg = match cfg.index_type {
                        IndexType::MACD | IndexType::MACDSignal => format!(
                            "📊 INDEX    | asset={} | {}_up={:.4} | {}_down={:.4} | pnl={:.4} | wins={} | losses={} | fund={:.4}",
                            asset_name, idx_name, ui, idx_name, di, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                        _ => format!(
                            "📊 INDEX    | asset={} | {}_up={:.2} | {}_down={:.2} | pnl={:.4} | wins={} | losses={} | fund={:.4}",
                            asset_name, idx_name, ui, idx_name, di, self.total_pnl, self.wins, self.losses, self.total_fund_used
                        ),
                    };
                    println!("{}", msg);
                    match cfg.index_type {
                        IndexType::MACD | IndexType::MACDSignal => info!(
                            "📊 {} Up={:.4} Down={:.4} | asset={} | pnl={:.4} wins={} losses={}",
                            idx_name, ui, di, asset_name, self.total_pnl, self.wins, self.losses
                        ),
                        _ => info!(
                            "📊 {} Up={:.2} Down={:.2} | asset={} | pnl={:.4} wins={} losses={}",
                            idx_name, ui, di, asset_name, self.total_pnl, self.wins, self.losses
                        ),
                    };
                    crate::log_trading_event(&msg);
                }
                _ => {
                    let msg = format!(
                        "📊 INDEX    | asset={} | {}=n/a | pnl={:.4} | wins={} | losses={} | fund={:.4}",
                        asset_name, idx_name, self.total_pnl, self.wins, self.losses, self.total_fund_used
                    );
                    println!("{}", msg);
                    info!(
                        "📊 Price update (no {} yet) for {} | pnl={:.4} wins={} losses={}",
                        idx_name, asset_name, self.total_pnl, self.wins, self.losses
                    );
                    crate::log_trading_event(&msg);
                }
            }
        }

        // We no longer use execute_action; all live logic is handled above.

        Ok(())
    }

    /// Run live trading loop
    pub async fn run(&mut self) -> anyhow::Result<()> {
        println!("🚀 Live trading mode started");
        println!("   Strategy      : {}", self.strategy.name());
        println!("   Markets       : {:?}", self.trading_assets);
        println!("   Initial equity: ${:.2}", self.capital);
        println!("   Check interval: {} ms", self.config.get_check_interval_ms());
        println!("   WARNING: Real order execution is NOT fully implemented yet!");
        info!("🚀 Starting LIVE TRADING MODE");
        info!("Strategy: {}", self.strategy.name());
        info!("Markets: {:?}", self.trading_assets);
        info!("Initial capital: ${:.2}", self.capital);
        info!("Check interval: {}ms", self.config.get_check_interval_ms());
        warn!("⚠️  WARNING: Real order execution is not yet fully implemented!");

        let check_interval = Duration::from_millis(self.config.get_check_interval_ms());

        loop {
            match self.monitor.fetch_market_data().await {
                Ok(snapshot) => {
                    if let Err(e) = self.process_snapshot(&snapshot).await {
                        error!("Error processing snapshot: {}", e);
                    }
                }
                Err(e) => {
                    error!("Error fetching market data: {}", e);
                }
            }

            sleep(check_interval).await;
        }
    }
}
