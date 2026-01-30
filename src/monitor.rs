// Market monitoring for real-time price data

use crate::api::PolymarketApi;
use crate::models::{Market, MarketData, TokenPrice};
use anyhow::Result;
use std::sync::Arc;
use tokio::time::Duration;
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    pub eth_market: MarketData,
    pub btc_market: MarketData,
    pub solana_market: MarketData,
    pub xrp_market: MarketData,
    pub timestamp: std::time::Instant,
    pub time_remaining_seconds: u64,
    pub period_timestamp: u64,
}

pub struct MarketMonitor {
    api: Arc<PolymarketApi>,
    eth_market: Arc<tokio::sync::Mutex<Market>>,
    btc_market: Arc<tokio::sync::Mutex<Market>>,
    solana_market: Arc<tokio::sync::Mutex<Market>>,
    xrp_market: Arc<tokio::sync::Mutex<Market>>,
    eth_up_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    eth_down_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    btc_up_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    btc_down_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    solana_up_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    solana_down_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    xrp_up_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    xrp_down_token_id: Arc<tokio::sync::Mutex<Option<String>>>,
    // Logging + enable flags
    enable_eth: bool,
    enable_solana: bool,
    enable_xrp: bool,
    eth_tokens_logged: Arc<tokio::sync::Mutex<bool>>,
    btc_tokens_logged: Arc<tokio::sync::Mutex<bool>>,
    /// Tracks which 15‑minute period we are currently trading (UNIX timestamp rounded to 900s)
    current_period_timestamp: Arc<tokio::sync::Mutex<u64>>,
}

impl MarketMonitor {
    pub fn new(
        api: Arc<PolymarketApi>,
        eth_market: Market,
        btc_market: Market,
        solana_market: Market,
        xrp_market: Market,
        enable_eth: bool,
        enable_solana: bool,
        enable_xrp: bool,
    ) -> Result<Self> {
        // Compute current 15‑minute period like polymarket‑trading‑bot
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let current_period = (current_time / 900) * 900;

        Ok(Self {
            api,
            eth_market: Arc::new(tokio::sync::Mutex::new(eth_market)),
            btc_market: Arc::new(tokio::sync::Mutex::new(btc_market)),
            solana_market: Arc::new(tokio::sync::Mutex::new(solana_market)),
            xrp_market: Arc::new(tokio::sync::Mutex::new(xrp_market)),
            eth_up_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            eth_down_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            btc_up_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            btc_down_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            solana_up_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            solana_down_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            xrp_up_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            xrp_down_token_id: Arc::new(tokio::sync::Mutex::new(None)),
            enable_eth,
            enable_solana,
            enable_xrp,
            eth_tokens_logged: Arc::new(tokio::sync::Mutex::new(false)),
            btc_tokens_logged: Arc::new(tokio::sync::Mutex::new(false)),
            current_period_timestamp: Arc::new(tokio::sync::Mutex::new(current_period)),
        })
    }

    /// Helper: round current UNIX timestamp down to the nearest 15‑minute period
    fn current_period(now: u64) -> u64 {
        (now / 900) * 900
    }

    /// Discover the active 15‑minute market for a given asset (ETH/BTC) by slug prefixes.
    ///
    /// This mirrors the logic in `src/bin/main.rs::discover_market`, but is local to the
    /// monitor so we can roll over to new markets when each 15‑minute period starts.
    async fn discover_market_for(
        &self,
        market_name: &str,
        slug_prefixes: &[&str],
        current_time: u64,
    ) -> anyhow::Result<Market> {
        let rounded_time = Self::current_period(current_time);

        for (i, prefix) in slug_prefixes.iter().enumerate() {
            if i > 0 {
                eprintln!("🔍 Trying {} market with slug prefix '{}'...", market_name, prefix);
            }

            // Try current period first
            let slug = format!("{}-updown-15m-{}", prefix, rounded_time);
            if let Ok(market) = self.api.get_market_by_slug(&slug).await {
                if market.active && !market.closed {
                    eprintln!(
                        "✅ Switched {} market to slug {} (condition_id={})",
                        market_name, market.slug, market.condition_id
                    );
                    return Ok(market);
                }
            }

            // Fallback: try a few previous periods in case of slight clock skew
            for offset in 1..=3 {
                let try_time = rounded_time.saturating_sub(offset * 900);
                let try_slug = format!("{}-updown-15m-{}", prefix, try_time);
                eprintln!("Trying previous {} market by slug: {}", market_name, try_slug);
                if let Ok(market) = self.api.get_market_by_slug(&try_slug).await {
                    if market.active && !market.closed {
                        eprintln!(
                            "✅ Switched {} market to slug {} (condition_id={})",
                            market_name, market.slug, market.condition_id
                        );
                        return Ok(market);
                    }
                }
            }
        }

        let tried = slug_prefixes.join(", ");
        anyhow::bail!(
            "Could not find active {} 15‑minute up/down market (tried prefixes: {})",
            market_name,
            tried
        )
    }

    /// If the 15‑minute period rolled over, discover the new ETH/BTC markets and reset
    /// token IDs so that we fetch prices for the new market's tokens instead of the
    /// previous (now closed) market.
    async fn maybe_roll_to_new_period(&self) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let new_period = Self::current_period(now);

        let mut period_lock = self.current_period_timestamp.lock().await;
        if *period_lock == new_period {
            // Same 15‑minute bucket, nothing to do.
            return Ok(());
        }

        eprintln!("🔄 Detected new 15‑minute period ({}) – rediscovering markets…", new_period);

        // Discover fresh ETH/BTC markets for the new period.
        // Even if ETH trading is disabled, we still track its market for completeness.
        let eth_market = self
            .discover_market_for("ETH", &["eth"], now)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to discover new ETH market: {}", e))?;
        let btc_market = self
            .discover_market_for("BTC", &["btc"], now)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to discover new BTC market: {}", e))?;

        {
            let mut eth_guard = self.eth_market.lock().await;
            *eth_guard = eth_market;
        }
        {
            let mut btc_guard = self.btc_market.lock().await;
            *btc_guard = btc_market;
        }

        // Reset token IDs so `refresh_tokens` will fetch IDs for the new markets.
        *self.eth_up_token_id.lock().await = None;
        *self.eth_down_token_id.lock().await = None;
        *self.btc_up_token_id.lock().await = None;
        *self.btc_down_token_id.lock().await = None;

        // Clear "logged" flags so token IDs for the new period are printed once.
        *self.eth_tokens_logged.lock().await = false;
        *self.btc_tokens_logged.lock().await = false;

        *period_lock = new_period;

        Ok(())
    }

    /// Refresh token IDs from CLOB market details (Up/Down token IDs)
    async fn refresh_tokens(&self) -> Result<()> {
        // Resolve current condition IDs
        let eth_condition_id = {
            let eth_guard = self.eth_market.lock().await;
            eth_guard.condition_id.clone()
        };
        let btc_condition_id = {
            let btc_guard = self.btc_market.lock().await;
            btc_guard.condition_id.clone()
        };

        // Fetch ETH market details and extract CLOB token IDs (only if ETH trading enabled)
        if self.enable_eth && eth_condition_id != "dummy_eth_fallback" {
            if let Ok(details) = self.api.get_market_details(&eth_condition_id).await {
                if let Some(tokens) = &details.tokens {
                    for token in tokens {
                        let outcome_upper = token.outcome.to_uppercase();
                        if outcome_upper.contains("UP") || outcome_upper == "1" {
                            let mut id_lock = self.eth_up_token_id.lock().await;
                            let first_time = id_lock.is_none();
                            *id_lock = Some(token.token_id.clone());
                            if first_time {
                                eprintln!("ETH Up token_id: {}", token.token_id);
                            }
                        } else if outcome_upper.contains("DOWN") || outcome_upper == "0" {
                            let mut id_lock = self.eth_down_token_id.lock().await;
                            let first_time = id_lock.is_none();
                            *id_lock = Some(token.token_id.clone());
                            if first_time {
                                eprintln!("ETH Down token_id: {}", token.token_id);
                            }
                        }
                    }
                }
            } else {
                eprintln!("⚠️  Failed to fetch ETH market details for condition_id {}", eth_condition_id);
            }
        }

        // Fetch BTC market details and extract CLOB token IDs
        if btc_condition_id != "dummy_btc_fallback" {
            if let Ok(details) = self.api.get_market_details(&btc_condition_id).await {
                if let Some(tokens) = &details.tokens {
                    for token in tokens {
                        let outcome_upper = token.outcome.to_uppercase();
                        if outcome_upper.contains("UP") || outcome_upper == "1" {
                            let mut id_lock = self.btc_up_token_id.lock().await;
                            let first_time = id_lock.is_none();
                            *id_lock = Some(token.token_id.clone());
                            if first_time {
                                eprintln!("BTC Up token_id: {}", token.token_id);
                            }
                        } else if outcome_upper.contains("DOWN") || outcome_upper == "0" {
                            let mut id_lock = self.btc_down_token_id.lock().await;
                            let first_time = id_lock.is_none();
                            *id_lock = Some(token.token_id.clone());
                            if first_time {
                                eprintln!("BTC Down token_id: {}", token.token_id);
                            }
                        }
                    }
                }
            } else {
                eprintln!("⚠️  Failed to fetch BTC market details for condition_id {}", btc_condition_id);
            }
        }

        Ok(())
    }

    /// Fetch current market data
    pub async fn fetch_market_data(&self) -> Result<MarketSnapshot> {
        // If the 15‑minute window has rolled over, switch to the new markets first
        // so that subsequent token‑ID refresh and price fetches use the new condition IDs.
        if let Err(e) = self.maybe_roll_to_new_period().await {
            eprintln!("⚠️  Failed to roll to new period markets: {}", e);
        }

        self.refresh_tokens().await?;

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let period_timestamp = (current_time / 900) * 900;
        let time_remaining_seconds = 900 - (current_time % 900);

        // Fetch prices for each market (mirroring polymarket-trading-bot)
        let eth_up_id = self.eth_up_token_id.lock().await.clone();
        let eth_down_id = self.eth_down_token_id.lock().await.clone();
        let btc_up_id = self.btc_up_token_id.lock().await.clone();
        let btc_down_id = self.btc_down_token_id.lock().await.clone();

        async fn fetch_token_price(
            api: &PolymarketApi,
            token_id: &Option<String>,
            market_name: &str,
            outcome: &str,
        ) -> Option<TokenPrice> {
            let token_id = token_id.as_ref()?;

            // BUY price (bid)
            let buy_price = match api.get_side_price(token_id, "BUY").await {
                Ok(price) => Some(price),
                Err(e) => {
                    eprintln!("⚠️  Failed to fetch {} {} BUY price: {}", market_name, outcome, e);
                    None
                }
            };

            // SELL price (ask)
            let sell_price = match api.get_side_price(token_id, "SELL").await {
                Ok(price) => Some(price),
                Err(e) => {
                    eprintln!("⚠️  Failed to fetch {} {} SELL price: {}", market_name, outcome, e);
                    None
                }
            };

            if buy_price.is_some() || sell_price.is_some() {
                Some(TokenPrice {
                    token_id: token_id.clone(),
                    bid: buy_price,
                    ask: sell_price,
                })
            } else {
                None
            }
        }

        // Fetch BTC prices (always enabled)
        let (btc_up_price, btc_down_price) = tokio::join!(
            fetch_token_price(&self.api, &btc_up_id, "BTC", "Up"),
            fetch_token_price(&self.api, &btc_down_id, "BTC", "Down"),
        );

        // Fetch ETH prices only if enabled
        let (eth_up_price, eth_down_price) = if self.enable_eth {
            tokio::join!(
                fetch_token_price(&self.api, &eth_up_id, "ETH", "Up"),
                fetch_token_price(&self.api, &eth_down_id, "ETH", "Down"),
            )
        } else {
            (None, None)
        };

        // --- Compact one-line log similar to polymarket-trading-bot ---
        fn fmt_token_price(tp: &Option<TokenPrice>) -> String {
            if let Some(tp) = tp {
                let bid = tp.bid.unwrap_or(Decimal::ZERO);
                let ask = tp.ask.unwrap_or(Decimal::ZERO);
                format!("${:.2}/${:.2}", bid, ask)
            } else {
                "$--/--".to_string()
            }
        }

        fn format_remaining_time(secs: u64) -> String {
            let mins = secs / 60;
            let rem = secs % 60;
            format!("{:2}m {:02}s", mins, rem)
        }

        use rust_decimal::Decimal;

        let btc_up_str = fmt_token_price(&btc_up_price);
        let btc_down_str = fmt_token_price(&btc_down_price);
        let eth_up_str = fmt_token_price(&eth_up_price);
        let eth_down_str = fmt_token_price(&eth_down_price);
        // For now, Solana/XRP are dummy - show N/A-style placeholders (and allow disabling later)
        let sol_up_str = "$--/--";
        let sol_down_str = "$--/--";
        let xrp_up_str = "$--/--";
        let xrp_down_str = "$--/--";

        let time_remaining_str = format_remaining_time(time_remaining_seconds);

        // Build log line conditionally based on enabled assets
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("BTC: U{} D{}", btc_up_str, btc_down_str));
        if self.enable_eth {
            parts.push(format!("ETH: U{} D{}", eth_up_str, eth_down_str));
        }
        if self.enable_solana {
            parts.push(format!("SOL: U{} D{}", sol_up_str, sol_down_str));
        }
        if self.enable_xrp {
            parts.push(format!("XRP: U{} D{}", xrp_up_str, xrp_down_str));
        }

        let price_log_line = format!("📊 {} | ⏱️  {}", parts.join(" | "), time_remaining_str);
        // Print to stdout so it's visible just like in polymarket-trading-bot
        println!("{}", price_log_line);
        // Also persist to history.toml
        crate::log_trading_event(&price_log_line);

        let eth_market_guard = self.eth_market.lock().await;
        let eth_market_data = MarketData {
            condition_id: eth_market_guard.condition_id.clone(),
            market_name: eth_market_guard.slug.clone(),
            up_token: eth_up_price,
            down_token: eth_down_price,
        };
        drop(eth_market_guard);

        let btc_market_guard = self.btc_market.lock().await;
        let btc_market_data = MarketData {
            condition_id: btc_market_guard.condition_id.clone(),
            market_name: btc_market_guard.slug.clone(),
            up_token: btc_up_price,
            down_token: btc_down_price,
        };
        drop(btc_market_guard);

        // Dummy data for Solana and XRP (can be enhanced later)
        let solana_market_data = MarketData {
            condition_id: "dummy".to_string(),
            market_name: "solana-updown-15m".to_string(),
            up_token: None,
            down_token: None,
        };

        let xrp_market_data = MarketData {
            condition_id: "dummy".to_string(),
            market_name: "xrp-updown-15m".to_string(),
            up_token: None,
            down_token: None,
        };

        Ok(MarketSnapshot {
            eth_market: eth_market_data,
            btc_market: btc_market_data,
            solana_market: solana_market_data,
            xrp_market: xrp_market_data,
            timestamp: std::time::Instant::now(),
            time_remaining_seconds,
            period_timestamp,
        })
    }

    /// Get Up token ID for an asset
    pub async fn get_up_token_id(&self, asset: &str) -> anyhow::Result<String> {
        match asset {
            "BTC" => {
                let guard = self.btc_up_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("BTC Up token ID not available. Market may not be initialized."))
            }
            "ETH" => {
                let guard = self.eth_up_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("ETH Up token ID not available. Market may not be initialized."))
            }
            "SOL" | "Solana" => {
                let guard = self.solana_up_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("Solana Up token ID not available. Market may not be initialized."))
            }
            "XRP" => {
                let guard = self.xrp_up_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("XRP Up token ID not available. Market may not be initialized."))
            }
            _ => anyhow::bail!("Unsupported asset: {}", asset),
        }
    }

    /// Get Down token ID for an asset
    pub async fn get_down_token_id(&self, asset: &str) -> anyhow::Result<String> {
        match asset {
            "BTC" => {
                let guard = self.btc_down_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("BTC Down token ID not available. Market may not be initialized."))
            }
            "ETH" => {
                let guard = self.eth_down_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("ETH Down token ID not available. Market may not be initialized."))
            }
            "SOL" | "Solana" => {
                let guard = self.solana_down_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("Solana Down token ID not available. Market may not be initialized."))
            }
            "XRP" => {
                let guard = self.xrp_down_token_id.lock().await;
                guard.clone().ok_or_else(|| anyhow::anyhow!("XRP Down token ID not available. Market may not be initialized."))
            }
            _ => anyhow::bail!("Unsupported asset: {}", asset),
        }
    }
}
