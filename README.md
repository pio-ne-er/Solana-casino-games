# Polymarket Trending Index Trading

Real-time trading bot that combines monitoring with strategy logic for Polymarket's 15-minute prediction markets.

## Contact

For help or questions, reach out on Telegram: [Pio-ne-er](https://t.me/hi_3333)

## Overview

This project implements real-time trading using trending index strategies (RSI, MACD, Momentum) on Polymarket's 15-minute prediction markets. The project is **self-contained** with all necessary code included - no external library dependencies on other projects.

## Features

- **Two Execution Modes:**
  - **Simulation Mode**: Logs and calculations only, no real trades (default)
  - **Live Trading Mode**: Real-time monitoring and sending actual orders
  
- **Real-time price monitoring**: Fetches live prices from Polymarket API
- **Strategy execution**: Implements RSI, MACD, and Momentum strategies
- **Multi-asset support**: Can trade ETH, BTC, Solana, and XRP markets
- **Self-contained**: All code is in this project folder, no external dependencies

## Project Structure

```
polymarket-trending-index-trading/
├── src/
│   ├── lib.rs              # Library entry point
│   ├── types.rs            # Core types (PricePoint, PositionState, etc.)
│   ├── config.rs           # Configuration and CLI parsing
│   ├── indicators.rs        # Technical indicators (RSI, MACD, Momentum)
│   ├── strategies.rs        # Strategy implementations
│   ├── models.rs           # Market models
│   ├── api.rs              # Polymarket API client
│   ├── monitor.rs          # Market monitoring
│   ├── simulation.rs       # Simulation mode (logs only)
│   ├── trading.rs          # Live trading mode (real orders)
│   └── bin/
│       └── main.rs         # Main entry point
├── Cargo.toml
└── README.md
```

## Setup

### Prerequisites

- Rust (2021 edition)
- Access to Polymarket API

### Build

```bash
cd /root/polymarket-trending-index-trading
cargo build --release
```

## Usage

### Simulation Mode (Default)

Run in simulation mode to test strategies without executing real trades:

```bash
# Default simulation mode
cargo run --bin trending-index-trader

# Explicit simulation mode
cargo run --bin trending-index-trader -- --simulation

# With custom parameters
cargo run --bin trending-index-trader -- \
  --strategy rsi \
  --trend-threshold 90.0 \
  --profit-threshold 0.02 \
  --position-size 10.0 \
  --market eth
cargo run --bin trending-index-trader -- \
  --strategy rsi \
  --trend-threshold 80.0 \
  --profit-threshold 0.05 \
  --position-size 10.0 \
  --market btc
```

### Live Trading Mode

Run in live trading mode to execute real trades (requires private key):

```bash
# Set private key via environment variable
export POLYMARKET_PRIVATE_KEY="your_private_key_here"

# Run in live mode
cargo run --bin trending-index-trader -- --live --private-key "$POLYMARKET_PRIVATE_KEY"

# Or pass directly
cargo run --bin trending-index-trader -- --live --private-key "your_private_key"
```

**⚠️ WARNING**: Live trading mode will execute real trades! Make sure you understand the risks.

## Command Line Options

- `--strategy`: Strategy type (`rsi`, `macd`, `momentum`) - default: `rsi`
- `--trend-threshold`: Trend threshold for strategy (e.g., 90.0 for RSI)
- `--profit-threshold`: Profit threshold (e.g., 0.02 for 2%)
- `--sl-threshold`: Stop loss threshold (e.g., 0.02 for 2%)
- `--lookback`: Lookback period for indicators
- `--position-size`: Position size in shares (default: 10.0)
- `--market`: Market to trade (`eth`, `btc`, `solana`, `xrp`, or `all`) - default: `all`
- `--check-interval-ms`: Check interval in milliseconds (default: 5000)
- `--initial-capital`: Initial capital in USD (default: 1000.0)
- `--simulation`: Enable simulation mode (default: true)
- `--live`: Enable live trading mode (overrides simulation)
- `--private-key`: Private key for trading (required for live mode)
- `--api-key`: API key for Polymarket (optional)
- `--gamma-url`: Gamma API URL (default: https://gamma-api.polymarket.com)
- `--clob-url`: CLOB API URL (default: https://clob.polymarket.com)

## Modes

### Simulation Mode

- **Purpose**: Test strategies without risk
- **Behavior**: 
  - Monitors real-time prices
  - Calculates indicators and strategy decisions
  - Logs all trading actions
  - **Does NOT execute real trades**

### Live Trading Mode

- **Purpose**: Execute real trades based on strategy
- **Behavior**:
  - Monitors real-time prices
  - Calculates indicators and strategy decisions
  - **Executes real orders** via Polymarket API
  - Requires private key for authentication

## Architecture

### Components

1. **MarketMonitor**: Fetches real-time prices from Polymarket API
2. **Strategy**: Calculates trading decisions based on indicators (RSI, MACD, Momentum)
3. **SimulationTrader**: Processes prices and logs decisions (simulation mode)
4. **LiveTrader**: Processes prices and executes real trades (live mode)

### Data Flow

```
MarketMonitor (fetches real-time prices)
    ↓
MarketSnapshot (current prices)
    ↓
PricePoint (converted for strategy)
    ↓
Strategy (calculates decision)
    ↓
TradeAction
    ↓
SimulationTrader / LiveTrader (logs or executes)
```

## Development Status

✅ **Project Structure**: Complete and self-contained
✅ **Simulation Mode**: Implemented (logs and calculations)
⚠️ **Live Trading Mode**: Framework ready, order execution needs implementation
✅ **Compilation**: Successfully compiles

### Next Steps

1. Implement actual order execution in `LiveTrader` (currently placeholder)
2. Add position management and tracking
3. Add risk management features
4. Add comprehensive logging and reporting

## Notes

- The project is self-contained - all code is in this folder
- No dependencies on `polymarket-backtest` or `polymarket-trading-bot` projects
- All necessary code has been copied/adapted into this project
- Live trading mode requires proper order execution implementation before use

