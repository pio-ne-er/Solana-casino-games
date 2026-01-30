# Configuration Guide

## Config.json File Support

The bot now supports loading configuration from a `config.json` file, similar to `polymarket-trading-bot`.

### Configuration Priority

Configuration values are loaded in this order (highest priority first):
1. **CLI arguments** (e.g., `--api-key`, `--private-key`)
2. **config.json file**
3. **Environment variables** (e.g., `POLYMARKET_API_KEY`)
4. **Default values**

### Config.json Format

Create a `config.json` file in the project root with the following structure:

```json
{
  "polymarket": {
    "gamma_api_url": "https://gamma-api.polymarket.com",
    "clob_api_url": "https://clob.polymarket.com",
    "api_key": "your_api_key_here",
    "api_secret": "your_api_secret_here",
    "api_passphrase": "your_api_passphrase_here",
    "private_key": "your_private_key_here",
    "proxy_wallet_address": "0xYourProxyWalletAddress",
    "signature_type": 2
  }
}
```

### Configuration Fields

- **gamma_api_url**: Gamma API endpoint (default: `https://gamma-api.polymarket.com`)
- **clob_api_url**: CLOB API endpoint (default: `https://clob.polymarket.com`)
- **api_key**: Polymarket API key
- **api_secret**: Polymarket API secret
- **api_passphrase**: Polymarket API passphrase
- **private_key**: Private key for signing transactions
- **proxy_wallet_address**: Proxy wallet address (for signature_type 1 or 2)
- **signature_type**: 
  - `0` = EOA (Externally Owned Account - private key account)
  - `1` = Proxy (Polymarket proxy wallet)
  - `2` = GnosisSafe (Gnosis Safe wallet)

### Usage Examples

#### Using config.json (default: config.json)
```bash
# Create config.json with your credentials
cp config.json.example config.json
# Edit config.json with your actual credentials

# Run with config.json
cargo run --bin trending-index-trader -- --simulation
```

#### Using custom config file
```bash
cargo run --bin trending-index-trader -- --config my-config.json --simulation
```

#### Overriding config.json with CLI arguments
```bash
# Use config.json but override API key
cargo run --bin trending-index-trader -- --api-key "override_key" --simulation
```

#### Using environment variables
```bash
export POLYMARKET_API_KEY="your_api_key"
export POLYMARKET_PRIVATE_KEY="your_private_key"
cargo run --bin trending-index-trader -- --simulation
```

### Example config.json

See `config.json.example` for a template. Copy it to `config.json` and fill in your credentials:

```bash
cp config.json.example config.json
# Edit config.json with your actual values
```

### Security Note

⚠️ **Important**: Never commit `config.json` to version control! It contains sensitive credentials.

Add to `.gitignore`:
```
config.json
```
