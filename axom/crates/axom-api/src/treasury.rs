//! The company's NEAR treasury, read-only: its account (where to deposit),
//! the network, and the balance NEAR reports. Nothing here moves funds or
//! touches a key; payouts are signed by axom-signer alone.
//!
//! Where the account comes from:
//! - Platform: the account stored when the company created its treasury.
//! - Single company: the signer, which derives it from its OpenBao key
//!   (`AXOM_SIGNER_URL`, e.g. `http://axom-signer:8090`). The signer is the
//!   only place that knows the key, so the address can't drift from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axom_core::near;
use axom_tools::near_rpc::{AccountBalance, JsonRpcNear};
use axom_tools::price::{HttpPriceFeed, PriceFeed};
use rust_decimal::Decimal;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::{admin::mfa_admin, reply, AppState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreasurySource {
    /// The account is known.
    Account(String),
    /// Ask the signer at this base URL (`GET /treasury`).
    Signer(String),
}

#[derive(Clone, Debug)]
pub struct TreasuryConfig {
    pub source: Option<TreasurySource>,
    /// `testnet` or `mainnet` (`NEAR_NETWORK`; testnet unless set to mainnet).
    pub network: String,
    pub rpc_url: String,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl TreasuryConfig {
    /// Single-company settings: the signer at `AXOM_SIGNER_URL`, if set.
    pub fn from_env() -> Self {
        Self::with_source(env("AXOM_SIGNER_URL").map(TreasurySource::Signer))
    }

    /// The network and RPC from the environment, with this source.
    pub fn with_source(source: Option<TreasurySource>) -> Self {
        let network = if env("NEAR_NETWORK").as_deref() == Some("mainnet") { "mainnet" } else { "testnet" };
        let rpc_url = env("NEAR_RPC_URL").unwrap_or_else(|| format!("https://rpc.{network}.fastnear.com"));
        Self { source, network: network.into(), rpc_url }
    }
}

/// Where to look an account up on the block explorer.
pub fn explorer_url(network: &str, account: &str) -> String {
    let host = if network == "mainnet" { "nearblocks.io" } else { "testnet.nearblocks.io" };
    format!("https://{host}/address/{account}")
}

/// The treasury account according to the signer. Refuses an answer for a
/// different network: a deposit address must never be shown under the
/// wrong network's label.
async fn from_signer(base: &str, network: &str) -> Result<String, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| e.to_string())?;
    let url = format!("{}/treasury", base.trim_end_matches('/'));
    let v: Value = match client.get(&url).send().await {
        Ok(r) => r.json().await.map_err(|_| "the payouts signer gave an unexpected answer".to_string())?,
        Err(e) => {
            // The dashboard says so on every load; one line per request is enough at debug.
            tracing::debug!(error = %e, "treasury: signer unreachable");
            return Err("the payouts signer isn't reachable, so the treasury address isn't known right now".into());
        }
    };
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        tracing::warn!(error = %e, "treasury: signer has no key");
        return Err("the payouts signer can't read the treasury key right now (is OpenBao sealed?)".into());
    }
    let theirs = v.get("network").and_then(Value::as_str).unwrap_or_default();
    if theirs != network {
        return Err(format!("the payouts signer is on NEAR {theirs}, but this server is on {network}; fix NEAR_NETWORK before depositing"));
    }
    let account = v.get("account").and_then(Value::as_str).unwrap_or_default().to_string();
    near::validate_account_id(&account).map_err(|_| "the payouts signer returned an invalid account".to_string())?;
    Ok(account)
}

/// The signer's account, remembered: it is derived from a key that doesn't
/// change, so the dashboard needn't wait on the signer for every load.
async fn signer_account(base: &str, network: &str) -> Result<String, String> {
    // Per signer and network, so a network mismatch is never hidden by an earlier answer.
    static KNOWN: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();
    let key = format!("{base} {network}");
    let known = KNOWN.get_or_init(Default::default);
    if let Some((a, t)) = known.lock().unwrap().get(&key) {
        if t.elapsed() < SIGNER_FRESH {
            return Ok(a.clone());
        }
    }
    let a = from_signer(base, network).await?;
    known.lock().unwrap().insert(key, (a.clone(), Instant::now()));
    Ok(a)
}
const SIGNER_FRESH: Duration = Duration::from_secs(10 * 60);

/// `GET /admin/treasury`: MFA-verified admins only.
pub async fn treasury(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = mfa_admin(&s, &headers).await {
        return *r;
    }
    let t = &s.treasury;
    let mut v = json!({
        "network": t.network,
        "configured": t.source.is_some(),
        "managed_by": match t.source { Some(TreasurySource::Signer(_)) => "signer", Some(TreasurySource::Account(_)) => "platform", None => "none" },
        "account": null,
        "explorer_url": null,
        "balance": null,
        "error": null,
    });
    let account = match &t.source {
        None => return reply(StatusCode::OK, v),
        Some(TreasurySource::Account(a)) => a.clone(),
        Some(TreasurySource::Signer(url)) => match signer_account(url, &t.network).await {
            Ok(a) => a,
            Err(e) => {
                v["error"] = json!(e);
                return reply(StatusCode::OK, v);
            }
        },
    };
    v["explorer_url"] = json!(explorer_url(&t.network, &account));
    v["account"] = json!(account);
    // Price and balance are independent lookups: ask both at once.
    let (price, bal) = tokio::join!(usd_per_near(&t.network), balance(&t.rpc_url, &account));
    v["price"] = price.as_ref().map(|p| json!({ "usd_per_near": p.usd_per_near.to_string(), "as_of": p.as_of, "pinned": p.pinned })).unwrap_or(Value::Null);
    match bal {
        Some((b, fetched)) => {
            let usd = |yocto: u128| price.as_ref().map(|p| near_decimal(yocto).checked_mul(p.usd_per_near).map(|u| u.round_dp(2).to_string()));
            v["balance"] = match b {
                Some(b) => json!({
                    "on_chain": true,
                    "near": near::yocto_to_near_string(b.amount_yocto),
                    "yocto": b.amount_yocto.to_string(),
                    "available_near": near::yocto_to_near_string(b.available_yocto()),
                    "locked_near": near::yocto_to_near_string(b.locked_yocto),
                    "usd": usd(b.amount_yocto).flatten(),
                }),
                // An implicit account exists once its first deposit lands.
                None => json!({ "on_chain": false, "near": "0", "yocto": "0", "available_near": "0", "locked_near": "0", "usd": usd(0).flatten() }),
            };
            v["balance"]["as_of"] = json!(fetched);
        }
        None => v["error"] = json!("couldn't read the balance from NEAR right now; try again shortly"),
    }
    reply(StatusCode::OK, v)
}

/// yocto -> NEAR with 8 decimals: enough for a dollar figure, and it fits a
/// Decimal however large the balance.
fn near_decimal(yocto: u128) -> Decimal {
    Decimal::from_i128_with_scale((yocto / 10u128.pow(16)) as i128, 8)
}

type Fetched = chrono::DateTime<chrono::Utc>;
/// A balance this recent is reused: the dashboard polls from several cards
/// and the public NEAR RPC rate-limits.
const BALANCE_FRESH: Duration = Duration::from_secs(15);
/// When NEAR can't be reached, the last balance is shown for this long.
const BALANCE_FALLBACK: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct BalanceEntry {
    last: Option<(Option<AccountBalance>, Fetched, Instant)>,
}

/// The account's balance (None inside = not on chain yet) and when it was
/// read, or None if NEAR can't be reached and there's no recent reading.
async fn balance(rpc_url: &str, account: &str) -> Option<(Option<AccountBalance>, Fetched)> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<BalanceEntry>>>>> = OnceLock::new();
    let entry = CACHE.get_or_init(Default::default).lock().unwrap().entry(format!("{rpc_url} {account}")).or_default().clone();
    // One lookup at a time per account; the others reuse its answer.
    let mut e = entry.lock().await;
    if let Some((b, at, t)) = &e.last {
        if t.elapsed() < BALANCE_FRESH {
            return Some((b.clone(), *at));
        }
    }
    match JsonRpcNear::with_timeout(rpc_url, Duration::from_secs(8)).view_account(account).await {
        Ok(b) => {
            let now = chrono::Utc::now();
            e.last = Some((b.clone(), now, Instant::now()));
            Some((b, now))
        }
        Err(err) => {
            tracing::warn!(error = %err, "treasury: balance lookup failed");
            e.last.as_ref().filter(|(_, _, t)| t.elapsed() < BALANCE_FALLBACK).map(|(b, at, _)| (b.clone(), *at))
        }
    }
}

#[derive(Clone)]
struct UsdPrice {
    usd_per_near: Decimal,
    as_of: Fetched,
    /// A fixed testnet rate (`AXOM_DEMO_USD_PER_NEAR`), not the market.
    pinned: bool,
}

/// How long a price is reused, and how long an old one is shown when the
/// feed can't be reached (the free CoinGecko tier allows few calls).
const PRICE_FRESH: Duration = Duration::from_secs(5 * 60);
const PRICE_FALLBACK: Duration = Duration::from_secs(60 * 60);

/// NEAR's USD price from the price feed (`NEAR_PRICE_FEED_*`, the same one
/// payouts use). On testnet, `AXOM_DEMO_USD_PER_NEAR` pins it if set.
async fn usd_per_near(network: &str) -> Option<UsdPrice> {
    if network == "testnet" {
        if let Some(p) = env("AXOM_DEMO_USD_PER_NEAR").and_then(|v| v.parse::<Decimal>().ok()).filter(|p| *p > Decimal::ZERO) {
            return Some(UsdPrice { usd_per_near: p, as_of: chrono::Utc::now(), pinned: true });
        }
    }
    static FEED: OnceLock<HttpPriceFeed> = OnceLock::new();
    static LAST: OnceLock<tokio::sync::Mutex<Option<(UsdPrice, Instant)>>> = OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().await;
    if let Some((p, t)) = last.as_ref() {
        if t.elapsed() < PRICE_FRESH {
            return Some(p.clone());
        }
    }
    match FEED.get_or_init(HttpPriceFeed::from_env).quote().await {
        Ok(q) => {
            let p = UsdPrice { usd_per_near: q.usd_per_near, as_of: q.fetched_at, pinned: false };
            *last = Some((p.clone(), Instant::now()));
            Some(p)
        }
        Err(e) => {
            tracing::warn!(error = %e, "treasury: NEAR price lookup failed");
            last.as_ref().filter(|(_, t)| t.elapsed() < PRICE_FALLBACK).map(|(p, _)| p.clone())
        }
    }
}

/// `GET /admin/treasury/yield`: treasury yield on Rhea lending, from what
/// the signer recorded (its latest plan and every move). MFA admins only;
/// read-only, and no NEAR RPC call (the signer reads the chain).
pub async fn treasury_yield(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let (cfg, _) = match crate::admin(&s, &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(r) = mfa_admin(&s, &headers).await {
        return *r;
    }
    let policy = axom_hr::company_policy::effective(&cfg.policy, &s.pool).await;
    let rules = policy.treasury().clone();
    let network = s.treasury.network.clone();
    let result = async {
        let status: Option<(Value, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT plan, checked_at FROM axom.treasury_status WHERE id = 1").fetch_optional(&s.pool).await?;
        type MoveRow = (uuid::Uuid, String, String, String, String, String, Option<String>, chrono::DateTime<chrono::Utc>);
        let moves: Vec<MoveRow> = sqlx::query_as(
            "SELECT id, kind, amount_yocto::text, reason, tx_hash, status, error, created_at FROM axom.treasury_moves
             WHERE network = $1 ORDER BY created_at DESC LIMIT 20",
        )
        .bind(&network)
        .fetch_all(&s.pool)
        .await?;
        let (supplied_in, withdrawn): (String, String) = sqlx::query_as(
            "SELECT COALESCE(SUM(amount_yocto) FILTER (WHERE kind = 'supply'), 0)::text,
                    COALESCE(SUM(amount_yocto) FILTER (WHERE kind = 'withdraw'), 0)::text
             FROM axom.treasury_moves WHERE network = $1 AND status = 'succeeded'",
        )
        .bind(&network)
        .fetch_one(&s.pool)
        .await?;
        Ok::<_, sqlx::Error>((status, moves, supplied_in, withdrawn))
    }
    .await;
    let (status, moves, supplied_in, withdrawn) = match result {
        Ok(v) => v,
        Err(e) => return crate::ApiError(e.to_string()).into_response(),
    };
    let y = |s: &str| s.parse::<u128>().unwrap_or(0);
    let supplied_now = status.as_ref().and_then(|(p, _)| p.pointer("/position/supplied_yocto").and_then(Value::as_str)).map(y);
    // Interest so far = what is in Rhea now - (supplied - withdrawn). Approximate:
    // a withdrawal larger than the principal includes interest already earned.
    let earned = supplied_now.map(|now| (now + y(&withdrawn)).saturating_sub(y(&supplied_in)));
    let explorer = |hash: &str| format!("https://{}nearblocks.io/txns/{hash}", if network == "mainnet" { "" } else { "testnet." });
    reply(
        StatusCode::OK,
        json!({
            "network": network,
            "protocol": "Rhea Finance lending",
            "lending_contract": axom_core::rhea::lending_contract(&network),
            "enabled": rules.yield_enabled,
            "rules": rules,
            "checked_at": status.as_ref().map(|s| s.1),
            "plan": status.map(|s| s.0),
            "earned_near": earned.map(near::yocto_to_near_string),
            "moves": moves.into_iter().map(|m| json!({
                "id": m.0, "kind": m.1, "amount_near": near::yocto_to_near_string(y(&m.2)), "reason": m.3,
                "tx_hash": m.4, "explorer_url": explorer(&m.4), "status": m.5, "error": m.6, "created_at": m.7,
            })).collect::<Vec<_>>(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_links_match_the_network() {
        assert_eq!(explorer_url("testnet", "a.testnet"), "https://testnet.nearblocks.io/address/a.testnet");
        assert_eq!(explorer_url("mainnet", "a.near"), "https://nearblocks.io/address/a.near");
    }

    #[test]
    fn usd_value_from_yocto() {
        // 2.5 NEAR at 3.10 USD.
        let yocto = 2_500_000_000_000_000_000_000_000u128;
        assert_eq!((near_decimal(yocto) * Decimal::new(310, 2)).round_dp(2).to_string(), "7.75");
        // Huge balances still fit.
        assert!(near_decimal(u128::MAX).checked_mul(Decimal::new(1, 0)).is_some());
        assert_eq!(near_decimal(0), Decimal::ZERO);
    }
}
