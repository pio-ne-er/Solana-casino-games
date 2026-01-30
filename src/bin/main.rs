// Main entry point for trending index trading bot

use anyhow::Result;
use polymarket_trending_index_trading::config::{CliConfig, Mode};
use polymarket_trending_index_trading::simulation::SimulationTrader;
use polymarket_trending_index_trading::trading::LiveTrader;
use polymarket_trending_index_trading::api::PolymarketApi;
use polymarket_trending_index_trading::monitor::MarketMonitor;
use polymarket_trending_index_trading::models::Market;
use polymarket_trending_index_trading::{init_history_file, log_trading_event};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::Arc;
use std::fs::OpenOptions;
use tracing::{info, error, warn};
use tracing_subscriber;

/// Discover market for a given asset
async fn discover_market(
    api: &PolymarketApi,
    market_name: &str,
    slug_prefixes: &[&str],
    current_time: u64,
) -> Result<Market> {
    let rounded_time = (current_time / 900) * 900; // Round to nearest 15 minutes

    for (i, prefix) in slug_prefixes.iter().enumerate() {
        if i > 0 {
            info!("🔍 Trying {} market with slug prefix '{}'...", market_name, prefix);
        }

        // Try current period with this prefix
        let slug = format!("{}-updown-15m-{}", prefix, rounded_time);
        if let Ok(market) = api.get_market_by_slug(&slug).await {
            if market.active && !market.closed {
                info!("✅ Found {} market by slug: {} | Condition ID: {}", market_name, market.slug, market.condition_id);
                return Ok(market);
            }
        }
    
        // Try previous periods with this prefix
        for offset in 1..=3 {
            let try_time = rounded_time - (offset * 900);
            let try_slug = format!("{}-updown-15m-{}", prefix, try_time);
            info!("Trying previous {} market by slug: {}", market_name, try_slug);
            if let Ok(market) = api.get_market_by_slug(&try_slug).await {
                if market.active && !market.closed {
                    info!("✅ Found {} market by slug: {} | Condition ID: {}", market_name, market.slug, market.condition_id);
                    return Ok(market);
                }
            }
        }
    }

    let tried = slug_prefixes.join(", ");
    anyhow::bail!(
        "Could not find active {} 15-minute up/down market (tried prefixes: {})",
        market_name,
        tried
    )
}

/// Create dummy market for fallback
fn create_dummy_market(name: &str, slug: &str) -> Market {
    Market {
        condition_id: format!("dummy_{}_fallback", name.to_lowercase()),
        market_id: None,
        question: format!("{} Up/Down 15m (Dummy)", name),
        slug: slug.to_string(),
        resolution_source: None,
        end_date_iso: None,
        active: false,
        closed: true,
        tokens: None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Open history.toml for append and initialize global history logger
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("history.toml")?;
    init_history_file(log_file);

    // Initialize logging (tracing to stderr)
    tracing_subscriber::fmt::init();

    // Parse CLI arguments
    let config = <CliConfig as clap::Parser>::parse();

    // Also print key info to stdout so you always see it without RUST_LOG
    println!("🚀 Starting Polymarket Trending Index Trading Bot");
    println!("📝 Logs are being saved to: history.toml");
    println!("Mode         : {:?}", config.mode());
    println!("Gamma URL    : {}", config.get_gamma_url());
    println!("CLOB URL     : {}", config.get_clob_url());
    println!("Check int.ms : {}", config.get_check_interval_ms());
    log_trading_event(&format!(
        "BOT START | mode={:?} | gamma_url={} | clob_url={} | check_interval_ms={}",
        config.mode(),
        config.get_gamma_url(),
        config.get_clob_url(),
        config.get_check_interval_ms()
    ));
    info!("🚀 Starting Polymarket Trending Index Trading Bot");
    info!("Mode: {:?}", config.mode());

    // Validate configuration
    if let Err(e) = config.validate() {
        error!("❌ Configuration error: {}", e);
        std::process::exit(1);
    }

    // Create API client
    let api = Arc::new(PolymarketApi::new(
        config.get_gamma_url(),
        config.get_clob_url(),
        config.get_api_key(),
        config.get_api_secret(),
        config.get_api_passphrase(),
        config.get_private_key(),
        config.get_proxy_wallet_address(),
        config.get_signature_type(),
    ));

    // Find current markets
    println!("🔍 Discovering current ETH/BTC markets (15m up/down)...");
    info!("🔍 Finding current markets...");
    
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Discover ETH market
    let eth_market = discover_market(&api, "ETH", &["eth"], current_time).await
        .unwrap_or_else(|e| {
            error!("❌ Could not find active ETH market: {}", e);
            std::process::exit(1);
        });

    // Discover BTC market
    let btc_market = discover_market(&api, "BTC", &["btc"], current_time).await
        .unwrap_or_else(|e| {
            error!("❌ Could not find active BTC market: {}", e);
            std::process::exit(1);
        });

    // Create dummy markets for Solana and XRP (can be enhanced later)
    let solana_market = create_dummy_market("Solana", "solana-updown-15m-dummy");
    let xrp_market = create_dummy_market("XRP", "xrp-updown-15m-dummy");

    println!("✅ Markets discovered:");
    println!("   ETH   : {} ({})", eth_market.slug, eth_market.condition_id);
    println!("   BTC   : {} ({})", btc_market.slug, btc_market.condition_id);
    println!("   Solana: {} ({})", solana_market.slug, solana_market.condition_id);
    println!("   XRP   : {} ({})", xrp_market.slug, xrp_market.condition_id);
    info!("✅ Found markets:");
    info!("   ETH: {} ({})", eth_market.slug, eth_market.condition_id);
    info!("   BTC: {} ({})", btc_market.slug, btc_market.condition_id);
    info!("   Solana: {} ({})", solana_market.slug, solana_market.condition_id);
    info!("   XRP: {} ({})", xrp_market.slug, xrp_market.condition_id);

    // Create market monitor (pass enable flags so it can skip/log per asset)
    let monitor = Arc::new(MarketMonitor::new(
        api.clone(),
        eth_market,
        btc_market,
        solana_market,
        xrp_market,
        config.is_eth_enabled(),
        config.is_solana_enabled(),
        config.is_xrp_enabled(),
    )?);

    // Get strategy configuration
    let strategy_config = config.get_strategy_config();
    println!(
        "Strategy cfg  : index={:?} | threshold={:.2} | mom_thresh={:.2}",
        strategy_config.index_type,
        strategy_config.trend_threshold,
        strategy_config.momentum_threshold_pct
    );
    info!(
        "Strategy config: index={:?}, threshold={:.2}, momentum_threshold_pct={:.2}",
        strategy_config.index_type,
        strategy_config.trend_threshold,
        strategy_config.momentum_threshold_pct
    );
    let initial_capital = Decimal::try_from(config.initial_capital)
        .unwrap_or(dec!(1000.0));

    // Run in appropriate mode
    match config.mode() {
        Mode::Simulation => {
            info!("🎮 Running in SIMULATION MODE (logs and calculations only)");
            let mut trader = SimulationTrader::new(
                monitor,
                strategy_config,
                config,
                initial_capital,
            );
            trader.run().await?;
        }
        Mode::Live => {
            info!("🚀 Running in LIVE TRADING MODE (monitoring and sending real orders)");
            warn!("⚠️  WARNING: Live trading mode will execute real trades!");
            let mut trader = LiveTrader::new(
                monitor,
                api,
                strategy_config,
                config,
                initial_capital,
            );
            trader.run().await?;
        }
    }

    Ok(())
}
