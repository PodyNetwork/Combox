//! NEAR Intents 1Click API client (the subset Axom uses) and a simulator.
//!
//! 1Click turns "deposit X of asset A here, deliver asset B to address R on
//! chain C" into an intent that solvers fill. Axom only ever uses:
//! - `POST /v0/quote`: a quote with a one-off deposit address;
//! - `POST /v0/deposit/submit`: tell 1Click the deposit transaction (optional, faster);
//! - `GET /v0/status`: execution status and destination-chain transactions;
//! - `GET /v0/tokens`: supported assets (for the dashboard and CLI).
//!
//! NEAR Intents has no testnet. On testnet Axom uses [`SimulatedIntents`]:
//! the NEAR leg (wrap + transfer to the deposit address on `wrap.testnet`)
//! is real; the solver side is simulated.
//!
//! Everything 1Click returns is untrusted data: the signer re-checks every
//! quote against the payout's signed authorization before depositing.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::price::{map_http_status, map_reqwest};
use crate::{ToolError, ToolResult};

pub const ONECLICK_URL: &str = "https://1click.chaindefuser.com";

/// `POST /v0/quote` body. Field names follow the 1Click OpenAPI spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub dry: bool,
    /// Always `EXACT_INPUT`: the deposit is the authorized yoctoNEAR amount.
    pub swap_type: String,
    /// Basis points.
    pub slippage_tolerance: u32,
    pub origin_asset: String,
    /// Always `ORIGIN_CHAIN`: the deposit is a transfer on NEAR.
    pub deposit_type: String,
    pub destination_asset: String,
    /// Smallest units of the origin asset (yoctoNEAR for wNEAR).
    pub amount: String,
    pub refund_to: String,
    pub refund_type: String,
    pub recipient: String,
    pub recipient_type: String,
    pub deadline: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referral: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_waiting_time_ms: Option<u64>,
    /// Not sent. USD/NEAR of the payout, used only by [`SimulatedIntents`]
    /// to price a fair fill.
    #[serde(skip)]
    pub reference_usd_per_near: Option<Decimal>,
}

impl QuoteRequest {
    /// An `EXACT_INPUT` wNEAR -> destination quote that refunds to the treasury on NEAR.
    #[allow(clippy::too_many_arguments)]
    pub fn payout(
        origin_asset: String,
        destination_asset: String,
        recipient: String,
        amount_yocto: u128,
        refund_to: String,
        slippage_bps: u32,
        deadline: DateTime<Utc>,
        dry: bool,
    ) -> Self {
        Self {
            dry,
            swap_type: "EXACT_INPUT".into(),
            slippage_tolerance: slippage_bps,
            origin_asset,
            deposit_type: "ORIGIN_CHAIN".into(),
            destination_asset,
            amount: amount_yocto.to_string(),
            refund_to,
            refund_type: "ORIGIN_CHAIN".into(),
            recipient,
            recipient_type: "DESTINATION_CHAIN".into(),
            deadline,
            referral: Some("axom".into()),
            quote_waiting_time_ms: Some(3000),
            reference_usd_per_near: None,
        }
    }
}

/// The parts of a quote response the signer checks, plus the raw response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quote {
    /// Absent on dry quotes.
    pub deposit_address: Option<String>,
    pub deposit_memo: Option<String>,
    pub amount_in: String,
    pub amount_out: String,
    pub amount_out_formatted: String,
    pub amount_out_usd: Option<Decimal>,
    pub min_amount_out: String,
    pub deadline: Option<DateTime<Utc>>,
    pub time_estimate_secs: Option<u64>,
    /// The request as 1Click echoes it back.
    pub echoed_request: Value,
    /// 1Click's signature over the quote; kept with the raw response.
    pub signature: String,
    pub raw: Value,
}

fn str_at(v: &Value, ptr: &str) -> Option<String> {
    match v.pointer(ptr)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Quote {
    pub fn parse(raw: Value) -> ToolResult<Self> {
        let need = |ptr: &str| str_at(&raw, ptr).ok_or_else(|| ToolError::Permanent(format!("1Click quote has no {ptr}")));
        Ok(Self {
            deposit_address: str_at(&raw, "/quote/depositAddress"),
            deposit_memo: str_at(&raw, "/quote/depositMemo"),
            amount_in: need("/quote/amountIn")?,
            amount_out: need("/quote/amountOut")?,
            amount_out_formatted: str_at(&raw, "/quote/amountOutFormatted").unwrap_or_default(),
            amount_out_usd: str_at(&raw, "/quote/amountOutUsd").and_then(|s| s.parse().ok()),
            min_amount_out: need("/quote/minAmountOut")?,
            deadline: str_at(&raw, "/quote/deadline").and_then(|s| s.parse().ok()),
            time_estimate_secs: raw.pointer("/quote/timeEstimate").and_then(Value::as_u64),
            echoed_request: raw.get("quoteRequest").cloned().unwrap_or(Value::Null),
            signature: str_at(&raw, "/signature").unwrap_or_default(),
            raw,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStatus {
    PendingDeposit,
    KnownDepositTx,
    IncompleteDeposit,
    Processing,
    Success,
    Refunded,
    Failed,
}

impl ExecutionStatus {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_string())).ok()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PendingDeposit => "PENDING_DEPOSIT",
            Self::KnownDepositTx => "KNOWN_DEPOSIT_TX",
            Self::IncompleteDeposit => "INCOMPLETE_DEPOSIT",
            Self::Processing => "PROCESSING",
            Self::Success => "SUCCESS",
            Self::Refunded => "REFUNDED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainTx {
    pub hash: String,
    pub explorer_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntentStatus {
    pub status: ExecutionStatus,
    pub destination_txs: Vec<ChainTx>,
    pub amount_out_formatted: Option<String>,
    pub refund_reason: Option<String>,
}

impl IntentStatus {
    pub fn parse(raw: &Value) -> ToolResult<Self> {
        let s = raw.get("status").and_then(Value::as_str).unwrap_or_default();
        let status = ExecutionStatus::parse(s).ok_or_else(|| ToolError::Permanent(format!("unknown 1Click status {s:?}")))?;
        let destination_txs = raw
            .pointer("/swapDetails/destinationChainTxHashes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(ChainTx {
                            hash: t.get("hash")?.as_str()?.to_string(),
                            explorer_url: t.get("explorerUrl").and_then(Value::as_str).map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            status,
            destination_txs,
            amount_out_formatted: str_at(raw, "/swapDetails/amountOutFormatted"),
            refund_reason: str_at(raw, "/swapDetails/refundReason"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentToken {
    #[serde(rename = "assetId")]
    pub asset_id: String,
    pub blockchain: String,
    pub symbol: String,
    pub decimals: u32,
    #[serde(default)]
    pub price: Option<f64>,
}

#[async_trait]
pub trait IntentsApi: Send + Sync {
    /// `simulated` or `1click`; shown in logs and the dashboard.
    fn mode(&self) -> &'static str;
    async fn quote(&self, req: &QuoteRequest) -> ToolResult<Quote>;
    /// Tell 1Click about the deposit transaction. Optional; speeds things up.
    async fn submit_deposit(&self, tx_hash: &str, deposit_address: &str, near_sender: &str) -> ToolResult<()>;
    async fn status(&self, deposit_address: &str) -> ToolResult<IntentStatus>;
    async fn tokens(&self) -> ToolResult<Vec<IntentToken>>;
}

// ---------------------------------------------------------------------------
// 1Click over HTTP
// ---------------------------------------------------------------------------

pub struct OneClickApi {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for OneClickApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("OneClickApi").field("base_url", &self.base_url).field("api_key", &self.api_key.as_ref().map(|_| "set")).finish()
    }
}

impl OneClickApi {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().expect("http client");
        Self { client, base_url: base_url.into().trim_end_matches('/').to_string(), api_key: api_key.filter(|k| !k.trim().is_empty()) }
    }

    /// `AXOM_INTENTS_API_URL` (default the public 1Click URL) and the optional
    /// partner key `AXOM_INTENTS_API_KEY` (without it 1Click adds its platform fee).
    pub fn from_env() -> Self {
        Self::new(std::env::var("AXOM_INTENTS_API_URL").unwrap_or_else(|_| ONECLICK_URL.into()), std::env::var("AXOM_INTENTS_API_KEY").ok())
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> ToolResult<Value> {
        let req = match &self.api_key {
            Some(k) => req.header("X-API-Key", k),
            None => req,
        };
        let resp = req.send().await.map_err(|e| map_reqwest(e, what))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = v.get("message").map(|m| m.to_string()).unwrap_or_default();
            return Err(match map_http_status(status, what) {
                ToolError::Permanent(m) => ToolError::Permanent(format!("{m} {msg}")),
                other => other,
            });
        }
        Ok(v)
    }
}

#[async_trait]
impl IntentsApi for OneClickApi {
    fn mode(&self) -> &'static str {
        "1click"
    }

    async fn quote(&self, req: &QuoteRequest) -> ToolResult<Quote> {
        let v = self.send(self.client.post(format!("{}/v0/quote", self.base_url)).json(req), "1click quote").await?;
        Quote::parse(v)
    }

    async fn submit_deposit(&self, tx_hash: &str, deposit_address: &str, near_sender: &str) -> ToolResult<()> {
        let body = json!({ "txHash": tx_hash, "depositAddress": deposit_address, "nearSenderAccount": near_sender });
        self.send(self.client.post(format!("{}/v0/deposit/submit", self.base_url)).json(&body), "1click deposit submit").await.map(|_| ())
    }

    async fn status(&self, deposit_address: &str) -> ToolResult<IntentStatus> {
        let v = self
            .send(self.client.get(format!("{}/v0/status", self.base_url)).query(&[("depositAddress", deposit_address)]), "1click status")
            .await?;
        IntentStatus::parse(&v)
    }

    async fn tokens(&self) -> ToolResult<Vec<IntentToken>> {
        let v = self.send(self.client.get(format!("{}/v0/tokens", self.base_url)), "1click tokens").await?;
        serde_json::from_value(v).map_err(|e| ToolError::Permanent(format!("1click tokens: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Simulator (testnet and tests)
// ---------------------------------------------------------------------------

/// Stands in for NEAR Intents where it does not exist (testnet) and in tests.
///
/// Quotes like a fair solver: the deposit address is a fresh NEAR implicit
/// account derived from the request, the output is the input's USD value
/// minus `fee_bps`, and the echoed request is the request. Every deposit
/// address it issued reports `SUCCESS` with a simulated destination hash.
/// Tests can override the status or tamper with the next quote.
pub struct SimulatedIntents {
    pub fee_bps: u32,
    state: Mutex<SimState>,
}

#[derive(Default)]
struct SimState {
    status_override: Option<ExecutionStatus>,
    tamper_recipient: Option<String>,
    out_usd_override: Option<Decimal>,
    quotes: Vec<QuoteRequest>,
    submitted: Vec<(String, String)>,
}

impl Default for SimulatedIntents {
    fn default() -> Self {
        Self { fee_bps: 30, state: Mutex::new(SimState::default()) }
    }
}

impl SimulatedIntents {
    pub fn new() -> Self {
        Self::default()
    }

    /// The deposit address the simulator issues for a request.
    pub fn deposit_address_for(req: &QuoteRequest) -> String {
        let mut h = Sha256::new();
        h.update(b"axom-simulated-intent-deposit:");
        h.update(serde_json::to_vec(req).expect("request serializes"));
        hex::encode(h.finalize())
    }

    pub fn set_status(&self, s: Option<ExecutionStatus>) {
        self.state.lock().unwrap().status_override = s;
    }
    /// The next quote names a different recipient than requested.
    pub fn tamper_next_recipient(&self, r: &str) {
        self.state.lock().unwrap().tamper_recipient = Some(r.to_string());
    }
    /// Report this USD output on every quote (e.g. a bad price).
    pub fn set_out_usd(&self, usd: Option<Decimal>) {
        self.state.lock().unwrap().out_usd_override = usd;
    }
    pub fn quotes(&self) -> Vec<QuoteRequest> {
        self.state.lock().unwrap().quotes.clone()
    }
    pub fn submitted(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().submitted.clone()
    }
}

#[async_trait]
impl IntentsApi for SimulatedIntents {
    fn mode(&self) -> &'static str {
        "simulated"
    }

    async fn quote(&self, req: &QuoteRequest) -> ToolResult<Quote> {
        let mut s = self.state.lock().unwrap();
        s.quotes.push(req.clone());
        let amount_in: u128 = req.amount.parse().map_err(|_| ToolError::Permanent("amount is not an integer".into()))?;
        let near_in = Decimal::from_i128_with_scale(amount_in as i128, 24);
        let usd_in = req.reference_usd_per_near.map(|p| near_in * p);
        let keep = Decimal::from(10_000 - self.fee_bps) / Decimal::from(10_000);
        let out_usd = s.out_usd_override.or(usd_in.map(|u| (u * keep).round_dp(6)));
        let mut echoed = serde_json::to_value(req).expect("request serializes");
        if let Some(r) = s.tamper_recipient.take() {
            echoed["recipient"] = Value::String(r);
        }
        let out = out_usd.unwrap_or_default();
        // Simulated 6-decimal stablecoin output.
        let out_units = (out * Decimal::from(1_000_000)).trunc().to_string();
        let min_out = (out * keep * Decimal::from(1_000_000)).trunc().to_string();
        let deposit = (!req.dry).then(|| Self::deposit_address_for(req));
        let raw = json!({
            "correlationId": "simulated",
            "timestamp": Utc::now(),
            "signature": "simulated",
            "quoteRequest": echoed,
            "quote": {
                "depositAddress": deposit,
                "amountIn": req.amount,
                "amountInFormatted": near_in.normalize().to_string(),
                "amountOut": out_units,
                "amountOutFormatted": out.normalize().to_string(),
                "amountOutUsd": out_usd.map(|u| u.to_string()),
                "minAmountOut": min_out,
                "deadline": (!req.dry).then_some(req.deadline),
                "timeEstimate": 20,
            }
        });
        Quote::parse(raw)
    }

    async fn submit_deposit(&self, tx_hash: &str, deposit_address: &str, _near_sender: &str) -> ToolResult<()> {
        self.state.lock().unwrap().submitted.push((tx_hash.to_string(), deposit_address.to_string()));
        Ok(())
    }

    async fn status(&self, deposit_address: &str) -> ToolResult<IntentStatus> {
        let s = self.state.lock().unwrap();
        let status = s.status_override.unwrap_or(ExecutionStatus::Success);
        let destination_txs = if status == ExecutionStatus::Success {
            let h = hex::encode(Sha256::digest(format!("axom-simulated-fill:{deposit_address}")));
            vec![ChainTx { hash: format!("simulated-{}", &h[..16]), explorer_url: None }]
        } else {
            vec![]
        };
        Ok(IntentStatus {
            status,
            destination_txs,
            amount_out_formatted: None,
            refund_reason: (status == ExecutionStatus::Refunded).then(|| "simulated refund".to_string()),
        })
    }

    async fn tokens(&self) -> ToolResult<Vec<IntentToken>> {
        Ok(vec![IntentToken {
            asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
            blockchain: "base".into(),
            symbol: "USDC".into(),
            decimals: 6,
            price: Some(1.0),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_request_matches_the_1click_schema() {
        let r = QuoteRequest::payout("nep141:wrap.near".into(), "nep141:btc.omft.near".into(), "bc1qxyz".into(), 10u128.pow(24), "treasury.near".into(), 100, "2026-11-14T12:00:00Z".parse().unwrap(), true);
        let v = serde_json::to_value(&r).unwrap();
        for k in ["dry", "swapType", "slippageTolerance", "originAsset", "depositType", "destinationAsset", "amount", "refundTo", "refundType", "recipient", "recipientType", "deadline"] {
            assert!(v.get(k).is_some(), "missing required field {k}");
        }
        assert_eq!(v["amount"], "1000000000000000000000000");
        assert_eq!(v["swapType"], "EXACT_INPUT");
        assert!(v.get("referenceUsdPerNear").is_none(), "simulator hint is never sent");
    }

    #[test]
    fn parses_quote_and_status_responses() {
        let raw = json!({
            "correlationId": "c", "timestamp": "2026-11-14T12:00:00Z", "signature": "ed25519:abc",
            "quoteRequest": { "recipient": "0xabc" },
            "quote": { "depositAddress": "dep.near", "amountIn": "100", "amountInFormatted": "0.0001", "amountInUsd": "0.0003",
                       "minAmountIn": "100", "amountOut": "290", "amountOutFormatted": "0.00029", "amountOutUsd": "0.00029",
                       "minAmountOut": "287", "timeEstimate": 120, "deadline": "2026-11-15T12:00:00Z" }
        });
        let q = Quote::parse(raw).unwrap();
        assert_eq!(q.deposit_address.as_deref(), Some("dep.near"));
        assert_eq!(q.amount_out_usd, Some("0.00029".parse().unwrap()));
        assert_eq!(q.echoed_request["recipient"], "0xabc");
        assert_eq!(q.time_estimate_secs, Some(120));
        assert!(Quote::parse(json!({ "quote": {} })).is_err());

        let st = IntentStatus::parse(&json!({
            "status": "SUCCESS",
            "swapDetails": { "destinationChainTxHashes": [{ "hash": "0xdead", "explorerUrl": "https://basescan.org/tx/0xdead" }], "amountOutFormatted": "4.98" }
        }))
        .unwrap();
        assert_eq!(st.status, ExecutionStatus::Success);
        assert_eq!(st.destination_txs[0].hash, "0xdead");
        assert_eq!(IntentStatus::parse(&json!({ "status": "REFUNDED", "swapDetails": {} })).unwrap().status, ExecutionStatus::Refunded);
        assert!(IntentStatus::parse(&json!({ "status": "WHATEVER" })).is_err());
    }

    #[tokio::test]
    async fn simulator_quotes_fairly() {
        let sim = SimulatedIntents::new();
        let mut r = QuoteRequest::payout("nep141:wrap.testnet".into(), "nep141:x".into(), "0xabc".into(), 2 * 10u128.pow(24), "t.testnet".into(), 100, Utc::now(), false);
        r.reference_usd_per_near = Some(Decimal::from(5));
        let q = sim.quote(&r).await.unwrap();
        assert_eq!(q.amount_out_usd, Some("9.97".parse().unwrap()));
        let dep = q.deposit_address.unwrap();
        assert!(axom_core::near::is_implicit_account(&dep));
        assert_eq!(sim.status(&dep).await.unwrap().status, ExecutionStatus::Success);
    }
}
