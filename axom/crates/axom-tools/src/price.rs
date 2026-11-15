//! NEAR/USD price feed. The URL and the JSON pointer to the price come from
//! the environment; the 15-minute staleness rule lives in the policy engine.

use async_trait::async_trait;
use axom_core::decision::PriceQuote;
use chrono::Utc;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;

use crate::{ToolError, ToolResult};

#[async_trait]
pub trait PriceFeed: Send + Sync {
    async fn quote(&self) -> ToolResult<PriceQuote>;
}

pub const DEFAULT_PRICE_URL: &str = "https://api.coingecko.com/api/v3/simple/price?ids=near&vs_currencies=usd";
pub const DEFAULT_PRICE_POINTER: &str = "/near/usd";

/// Fetches a JSON document and reads the price at a JSON pointer.
pub struct HttpPriceFeed {
    client: reqwest::Client,
    url: String,
    pointer: String,
    /// Optional API key header (CoinGecko: `x-cg-demo-api-key` or `x-cg-pro-api-key`).
    api_key: Option<(String, String)>,
}

impl HttpPriceFeed {
    pub fn new(url: impl Into<String>, pointer: impl Into<String>) -> Self {
        // CoinGecko answers 403 to requests without a User-Agent.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("combox/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Self { client, url: url.into(), pointer: pointer.into(), api_key: None }
    }

    pub fn with_api_key(mut self, header: impl Into<String>, key: impl Into<String>) -> Self {
        self.api_key = Some((header.into(), key.into()));
        self
    }

    /// From `NEAR_PRICE_FEED_URL` and `NEAR_PRICE_FEED_POINTER`, with CoinGecko
    /// defaults, plus `NEAR_PRICE_FEED_API_KEY` sent in the header named by
    /// `NEAR_PRICE_FEED_API_KEY_HEADER` (default `x-cg-demo-api-key`).
    pub fn from_env() -> Self {
        let feed = Self::new(
            std::env::var("NEAR_PRICE_FEED_URL").unwrap_or_else(|_| DEFAULT_PRICE_URL.into()),
            std::env::var("NEAR_PRICE_FEED_POINTER").unwrap_or_else(|_| DEFAULT_PRICE_POINTER.into()),
        );
        match std::env::var("NEAR_PRICE_FEED_API_KEY").ok().filter(|k| !k.is_empty()) {
            Some(key) => feed.with_api_key(std::env::var("NEAR_PRICE_FEED_API_KEY_HEADER").unwrap_or_else(|_| "x-cg-demo-api-key".into()), key),
            None => feed,
        }
    }
}

/// Parse a price from a JSON document. Strings and numbers are accepted.
pub fn parse_price(doc: &serde_json::Value, pointer: &str) -> ToolResult<Decimal> {
    let v = doc.pointer(pointer).ok_or_else(|| ToolError::Permanent(format!("price feed has no value at {pointer}")))?;
    let price = match v {
        serde_json::Value::Number(n) => n.as_f64().and_then(Decimal::from_f64),
        serde_json::Value::String(s) => s.parse::<Decimal>().ok(),
        _ => None,
    }
    .ok_or_else(|| ToolError::Permanent(format!("price at {pointer} is not a number")))?
    .round_dp(6);
    if price <= Decimal::ZERO {
        return Err(ToolError::Permanent(format!("non-positive price {price}")));
    }
    Ok(price)
}

pub(crate) fn map_http_status(status: reqwest::StatusCode, what: &str) -> ToolError {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        ToolError::RateLimited { retry_after_secs: 60 }
    } else if status.is_server_error() {
        ToolError::Retryable(format!("{what}: HTTP {status}"))
    } else {
        ToolError::Permanent(format!("{what}: HTTP {status}"))
    }
}

pub(crate) fn map_reqwest(e: reqwest::Error, what: &str) -> ToolError {
    // reqwest errors can include the URL; ours carry no credentials, but keep
    // messages short and generic anyway.
    // A body that broke off or isn't what was expected is a transport
    // problem too, not a verdict on the request.
    if e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode() {
        ToolError::Retryable(format!("{what}: network error"))
    } else {
        ToolError::Permanent(format!("{what}: {}", e.without_url()))
    }
}

#[async_trait]
impl PriceFeed for HttpPriceFeed {
    async fn quote(&self) -> ToolResult<PriceQuote> {
        let mut req = self.client.get(&self.url);
        if let Some((h, k)) = &self.api_key {
            req = req.header(h.as_str(), k.as_str());
        }
        let resp = req.send().await.map_err(|e| map_reqwest(e, "price feed"))?;
        if !resp.status().is_success() {
            return Err(map_http_status(resp.status(), "price feed"));
        }
        let doc: serde_json::Value = resp.json().await.map_err(|e| map_reqwest(e, "price feed"))?;
        Ok(PriceQuote { usd_per_near: parse_price(&doc, &self.pointer)?, fetched_at: Utc::now() })
    }
}

/// Fixed-price feed for tests and judge mode.
pub struct FixedPriceFeed(pub PriceQuote);

#[async_trait]
impl PriceFeed for FixedPriceFeed {
    async fn quote(&self) -> ToolResult<PriceQuote> {
        Ok(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    #[test]
    fn parses_coingecko_and_strings() {
        assert_eq!(parse_price(&json!({ "near": { "usd": 3.21 } }), "/near/usd").unwrap(), dec!(3.21));
        assert_eq!(parse_price(&json!({ "price": "4.5" }), "/price").unwrap(), dec!(4.5));
        assert!(parse_price(&json!({ "near": {} }), "/near/usd").is_err());
        assert!(parse_price(&json!({ "p": -1 }), "/p").is_err());
    }
}
