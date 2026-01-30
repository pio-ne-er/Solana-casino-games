// Simplified Polymarket API client

use crate::models::{Market, MarketDetails, OrderRequest, OrderResponse};
use anyhow::{Result, Context};
use reqwest::Client;
use serde_json::Value;
use rust_decimal::Decimal;
use std::str::FromStr;

// Polymarket SDK imports for order placement
use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk::clob::types::{Side, SignatureType};
use polymarket_client_sdk::POLYGON;
use alloy::signers::local::LocalSigner;
use alloy::signers::Signer as _;
use alloy::primitives::Address as AlloyAddress;
use polymarket_client_sdk::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk::clob::types::AssetType;

pub struct PolymarketApi {
    client: Client,
    gamma_url: String,
    clob_url: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    api_passphrase: Option<String>,
    private_key: Option<String>,
    proxy_wallet_address: Option<String>,
    signature_type: Option<u8>,
}

impl PolymarketApi {
    pub fn new(
        gamma_url: String,
        clob_url: String,
        api_key: Option<String>,
        api_secret: Option<String>,
        api_passphrase: Option<String>,
        private_key: Option<String>,
        proxy_wallet_address: Option<String>,
        signature_type: Option<u8>,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            client,
            gamma_url,
            clob_url,
            api_key,
            api_secret,
            api_passphrase,
            private_key,
            proxy_wallet_address,
            signature_type,
        }
    }

    /// Get market details by condition ID (CLOB /markets/{conditionId})
    /// Used to resolve CLOB token IDs and prices for Up/Down outcomes.
    pub async fn get_market_details(&self, condition_id: &str) -> Result<MarketDetails> {
        let url = format!("{}/markets/{}", self.clob_url, condition_id);
        let mut request = self.client.get(&url);

        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!(
                "Failed to fetch market details for condition_id {} (status: {})",
                condition_id,
                status
            );
        }

        let json_text = response.text().await?;
        let market: MarketDetails = serde_json::from_str(&json_text)?;
        Ok(market)
    }

    /// Get market by slug (e.g., "eth-updown-15m-1767726000")
    ///
    /// NOTE: Polymarket's Gamma API returns an *event* object for an event slug,
    /// which contains a `markets` array. We need to fetch `/events/slug/{slug}`
    /// and then extract the first market from that array (same as polymarket-trading-bot).
    pub async fn get_market_by_slug(&self, slug: &str) -> Result<Market> {
        // IMPORTANT: use /events/slug/{slug}, not /markets/{slug}
        let url = format!("{}/events/slug/{}", self.gamma_url, slug);

        let mut request = self.client.get(&url);

        // Add API key header if available
        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!(
                "Failed to fetch market by slug: {} (status: {})",
                slug,
                status
            );
        }

        let json: Value = response.json().await?;

        // Response is an event object with a "markets" array
        if let Some(markets) = json.get("markets").and_then(|m| m.as_array()) {
            if let Some(market_json) = markets.first() {
                if let Ok(market) = serde_json::from_value::<Market>(market_json.clone()) {
                    return Ok(market);
                }
            }
        }

        anyhow::bail!("Invalid market response format for slug {}: no markets array found", slug)
    }

    /// Get single-side price for a token (mirrors polymarket-trading-bot)
    /// side: "BUY" (bid) or "SELL" (ask)
    pub async fn get_side_price(&self, token_id: &str, side: &str) -> Result<Decimal> {
        let url = format!("{}/price", self.clob_url);
        let mut request = self.client.get(&url).query(&[("side", side), ("token_id", token_id)]);

        // Add API key header if available
        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Failed to fetch price for token {} side {} (status: {})", token_id, side, status);
        }

        let json: Value = response.json().await?;
        let price_str = json
            .get("price")
            .and_then(|p| p.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid price response format for token {}", token_id))?;

        let price = Decimal::from_str(price_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse price {} for token {}: {}", price_str, token_id, e))?;

        Ok(price)
    }

    /// Place an order using the official Polymarket SDK
    /// This method creates, signs, and posts orders to the CLOB
    pub async fn place_order(&self, order: &OrderRequest) -> Result<OrderResponse> {
        let private_key = self.private_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Private key required for order signing"))?;
        
        // Create signer from private key
        let signer = LocalSigner::from_str(private_key)
            .context("Failed to create signer from private key. Ensure private_key is a valid hex string.")?
            .with_chain_id(Some(POLYGON));
        
        // Build authentication builder
        let mut auth_builder = ClobClient::new(&self.clob_url, ClobConfig::default())
            .context("Failed to create CLOB client")?
            .authentication_builder(&signer);
        
        // Configure proxy wallet if provided
        if let Some(proxy_addr) = &self.proxy_wallet_address {
            let funder_address = AlloyAddress::parse_checksummed(proxy_addr, None)
                .context(format!("Failed to parse proxy_wallet_address: {}. Ensure it's a valid Ethereum address.", proxy_addr))?;
            
            auth_builder = auth_builder.funder(funder_address);
            
            // Set signature type based on config
            let sig_type = match self.signature_type {
                Some(1) => SignatureType::Proxy,
                Some(2) => SignatureType::GnosisSafe,
                Some(0) | None => SignatureType::Proxy, // Default to Proxy when proxy wallet is set
                Some(n) => anyhow::bail!("Invalid signature_type: {}. Must be 0 (EOA), 1 (Proxy), or 2 (GnosisSafe)", n),
            };
            
            auth_builder = auth_builder.signature_type(sig_type);
        } else if let Some(sig_type_num) = self.signature_type {
            // If signature type is set but no proxy wallet, validate it's EOA
            let sig_type = match sig_type_num {
                0 => SignatureType::Eoa,
                1 | 2 => anyhow::bail!("signature_type {} requires proxy_wallet_address to be set", sig_type_num),
                n => anyhow::bail!("Invalid signature_type: {}. Must be 0 (EOA), 1 (Proxy), or 2 (GnosisSafe)", n),
            };
            auth_builder = auth_builder.signature_type(sig_type);
        }
        
        // Authenticate with CLOB API
        let client = auth_builder
            .authenticate()
            .await
            .context("Failed to authenticate with CLOB API. Check your API credentials (api_key, api_secret, api_passphrase).")?;
        
        // Convert order side string to SDK Side enum
        let side = match order.side.as_str() {
            "BUY" => Side::Buy,
            "SELL" => Side::Sell,
            _ => anyhow::bail!("Invalid order side: {}. Must be 'BUY' or 'SELL'", order.side),
        };
        
        // Parse price and size to Decimal
        let price = Decimal::from_str(&order.price)
            .context(format!("Failed to parse price: {}", order.price))?;
        let size = Decimal::from_str(&order.size)
            .context(format!("Failed to parse size: {}", order.size))?;
        
        // Create and sign order using SDK
        let order_builder = client
            .limit_order()
            .token_id(&order.token_id)
            .size(size)
            .price(price)
            .side(side);
        
        let signed_order = client.sign(&signer, order_builder.build().await?)
            .await
            .context("Failed to sign order")?;
        
        // Post order to CLOB
        let response = match client.post_order(signed_order).await {
            Ok(resp) => resp,
            Err(e) => {
                anyhow::bail!("Failed to post order: {}", e);
            }
        };
        
        // Check if the response indicates failure
        if !response.success {
            let error_msg = response.error_msg.as_deref().unwrap_or("Unknown error");
            anyhow::bail!("Order was rejected: {}", error_msg);
        }
        
        // Convert SDK response to our OrderResponse format
        Ok(OrderResponse {
            success: response.success,
            order_id: Some(response.order_id.clone()),
            status: Some(response.status.to_string()),
            message: Some(format!("Order placed successfully. Order ID: {}", response.order_id)),
            error_msg: response.error_msg,
        })
    }

    /// Cancel an order by order ID
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let private_key = self.private_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Private key required for order cancellation"))?;
        
        // Create signer from private key
        let signer = LocalSigner::from_str(private_key)
            .context("Failed to create signer from private key")?
            .with_chain_id(Some(POLYGON));
        
        // Build authentication builder
        let mut auth_builder = ClobClient::new(&self.clob_url, ClobConfig::default())
            .context("Failed to create CLOB client")?
            .authentication_builder(&signer);
        
        // Configure proxy wallet if provided
        if let Some(proxy_addr) = &self.proxy_wallet_address {
            let funder_address = AlloyAddress::parse_checksummed(proxy_addr, None)
                .context("Failed to parse proxy_wallet_address")?;
            
            auth_builder = auth_builder.funder(funder_address);
            
            let sig_type = match self.signature_type {
                Some(1) => SignatureType::Proxy,
                Some(2) => SignatureType::GnosisSafe,
                Some(0) | None => SignatureType::Proxy,
                _ => SignatureType::Proxy,
            };
            auth_builder = auth_builder.signature_type(sig_type);
        } else if let Some(sig_type_num) = self.signature_type {
            let sig_type = match sig_type_num {
                0 => SignatureType::Eoa,
                _ => SignatureType::Eoa,
            };
            auth_builder = auth_builder.signature_type(sig_type);
        }
        
        // Authenticate with CLOB API
        let client = auth_builder
            .authenticate()
            .await
            .context("Failed to authenticate with CLOB API")?;
        
        // Cancel the order
        client.cancel_order(order_id).await
            .context(format!("Failed to cancel order {}", order_id))?;
        
        Ok(())
    }

    /// Check conditional token balance only (shares) for a token_id.
    ///
    /// Used in LIVE mode to confirm entry fills by observing real balance changes.
    pub async fn check_balance_only(&self, token_id: &str) -> Result<Decimal> {
        let private_key = self.private_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Private key required for balance check"))?;

        // Create signer from private key
        let signer = LocalSigner::from_str(private_key)
            .context("Failed to create signer from private key. Ensure private_key is a valid hex string.")?
            .with_chain_id(Some(POLYGON));

        // Build authentication builder
        let mut auth_builder = ClobClient::new(&self.clob_url, ClobConfig::default())
            .context("Failed to create CLOB client")?
            .authentication_builder(&signer);

        // Configure proxy wallet if provided
        if let Some(proxy_addr) = &self.proxy_wallet_address {
            let funder_address = AlloyAddress::parse_checksummed(proxy_addr, None)
                .context(format!("Failed to parse proxy_wallet_address: {}. Ensure it's a valid Ethereum address.", proxy_addr))?;
            auth_builder = auth_builder.funder(funder_address);

            let sig_type = match self.signature_type {
                Some(1) => SignatureType::Proxy,
                Some(2) => SignatureType::GnosisSafe,
                Some(0) | None => SignatureType::Proxy,
                Some(n) => anyhow::bail!("Invalid signature_type: {}. Must be 0 (EOA), 1 (Proxy), or 2 (GnosisSafe)", n),
            };
            auth_builder = auth_builder.signature_type(sig_type);
        } else if let Some(sig_type_num) = self.signature_type {
            let sig_type = match sig_type_num {
                0 => SignatureType::Eoa,
                1 | 2 => anyhow::bail!("signature_type {} requires proxy_wallet_address to be set", sig_type_num),
                n => anyhow::bail!("Invalid signature_type: {}. Must be 0 (EOA), 1 (Proxy), or 2 (GnosisSafe)", n),
            };
            auth_builder = auth_builder.signature_type(sig_type);
        }

        // Authenticate with CLOB API
        let client = auth_builder
            .authenticate()
            .await
            .context("Failed to authenticate with CLOB API for balance check")?;

        let request = BalanceAllowanceRequest::builder()
            .token_id(token_id.to_string())
            .asset_type(AssetType::Conditional)
            .build();

        let balance_allowance = client
            .balance_allowance(request)
            .await
            .context("Failed to fetch balance")?;

        Ok(balance_allowance.balance)
    }
}
