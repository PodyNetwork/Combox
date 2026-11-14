//! axom-signer service: every 10 seconds, send approved payouts (after its
//! own checks) and finalize sent ones (cross-chain payouts once NEAR Intents
//! reports delivery). About once a minute, when no payout went out that
//! tick, it also manages treasury yield on Rhea lending (see
//! `axom_signer::treasury`). Serves GET /health and /treasury.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context;
use axom_core::near::PublicKey;
use axom_hr::notify::NotifyConfig;
use axom_hr::PolicyConfig;
use axom_signer::openbao::{OpenBaoTransit, TransitSigner};
use axom_signer::{Signer, SignerConfig};
use axom_tools::intents::{IntentsApi, OneClickApi, SimulatedIntents};
use axom_tools::near_rpc::{testnet_rpc_url, JsonRpcNear};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// NEAR/USD for the treasury planner, refreshed at most once a minute:
/// the judge-mode pinned rate (testnet only), the price feed, or 1Click's
/// own wNEAR price.
async fn near_price(cache: &mut Option<(std::time::Instant, axom_hr::PriceQuote)>, network: &str) -> Option<axom_hr::PriceQuote> {
    use axom_tools::price::{HttpPriceFeed, PriceFeed};
    if let Some((at, q)) = cache {
        if at.elapsed() < std::time::Duration::from_secs(60) {
            return Some(*q);
        }
    }
    let now = chrono::Utc::now();
    let pinned = (network != "mainnet")
        .then(|| std::env::var("AXOM_DEMO_USD_PER_NEAR").ok().and_then(|v| v.parse::<rust_decimal::Decimal>().ok()))
        .flatten();
    let q = match pinned {
        Some(p) => Some(axom_hr::PriceQuote { usd_per_near: p, fetched_at: now }),
        None => match HttpPriceFeed::from_env().quote().await {
            Ok(q) => Some(q),
            Err(_) => OneClickApi::from_env().tokens().await.ok().and_then(|t| {
                t.iter()
                    .find(|t| t.asset_id == "nep141:wrap.near")
                    .and_then(|t| t.price)
                    .and_then(rust_decimal::Decimal::from_f64_retain)
                    .map(|p| axom_hr::PriceQuote { usd_per_near: p.round_dp(6), fetched_at: now })
            }),
        },
    };
    if let Some(q) = q {
        *cache = Some((std::time::Instant::now(), q));
    }
    q
}

fn log_treasury(company: &str, r: anyhow::Result<axom_signer::treasury::TreasuryResult>) {
    use axom_signer::treasury::TreasuryResult;
    match r {
        Ok(TreasuryResult::Held { reason }) => tracing::debug!(%company, %reason, "treasury: holding"),
        Ok(r) => tracing::info!(%company, result = ?r, "treasury"),
        Err(e) => tracing::error!(%company, error = %e, "treasury cycle failed"),
    }
}

fn env_key(var: &str) -> anyhow::Result<PublicKey> {
    let v = std::env::var(var).with_context(|| format!("{var} is not set"))?;
    PublicKey::parse(&v).with_context(|| format!("{var} must be ed25519:<base58>"))
}

/// Cross-chain payouts (NEAR Intents). 1Click exists only on mainnet; on
/// testnet the NEAR leg is real and the solver side is simulated.
fn intents_from_env(network: &str) -> anyhow::Result<Option<Box<dyn IntentsApi>>> {
    let mode = std::env::var("AXOM_INTENTS").unwrap_or_else(|_| if network == "mainnet" { "1click".into() } else { "simulated".into() });
    let intents: Option<Box<dyn IntentsApi>> = match (mode.as_str(), network) {
        ("off", _) => None,
        ("1click", "mainnet") => Some(Box::new(OneClickApi::from_env())),
        ("1click", _) => anyhow::bail!("AXOM_INTENTS=1click needs NEAR_NETWORK=mainnet: NEAR Intents has no testnet"),
        ("simulated", "mainnet") => anyhow::bail!("AXOM_INTENTS=simulated is for testnet only"),
        ("simulated", _) => Some(Box::new(SimulatedIntents::new())),
        (other, _) => anyhow::bail!("AXOM_INTENTS must be 1click, simulated, or off (got {other:?})"),
    };
    tracing::info!(mode = intents.as_ref().map(|i| i.mode()).unwrap_or("off"), "cross-chain payouts (NEAR Intents)");
    Ok(intents)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let network = SignerConfig::network_from_env();
    if network == "mainnet" && std::env::var("AXOM_LIVE").as_deref() != Ok("true") {
        anyhow::bail!("NEAR_NETWORK=mainnet requires AXOM_LIVE=true (after the owner's legal review)");
    }
    let rpc_url = match network.as_str() {
        "mainnet" => std::env::var("NEAR_RPC_URL").context("NEAR_RPC_URL must be set explicitly for mainnet")?,
        _ => testnet_rpc_url(),
    };
    let cfg = SignerConfig {
        network: network.clone(),
        attestation_key: env_key("AXOM_ATTESTATION_VERIFY_KEY")?,
        approval_key: env_key("AXOM_APPROVAL_VERIFY_KEY")?,
        max_attempts: 5,
        base_backoff: chrono::Duration::seconds(30),
    };
    let policy = PolicyConfig::load(&PathBuf::from(std::env::var("AXOM_POLICY_PATH").unwrap_or_else(|_| "config/policy.toml".into())))?;
    if std::env::var("AXOM_MULTI_TENANT").as_deref() == Ok("true") {
        let intents = intents_from_env(&network)?;
        return multi_tenant(cfg, policy, rpc_url, intents).await;
    }
    let transit = OpenBaoTransit::from_env()?;
    let near = JsonRpcNear::new(rpc_url);
    let notify_cfg = NotifyConfig::from_env();
    let intents = intents_from_env(&network)?;

    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let opts = PgConnectOptions::from_str(&db_url)?.options([("search_path", "public")]);
    let pool = PgPoolOptions::new().max_connections(5).connect_with(opts).await?;
    axom_core::MIGRATOR.run(&pool).await?;

    match transit.public_key().await {
        Ok(pk) => tracing::info!(%network, treasury = %pk.implicit_account(), "treasury key loaded from OpenBao"),
        Err(e) => tracing::warn!(error = %e, "treasury key unavailable (sealed?); payouts paused until it is"),
    }

    let bind = std::env::var("AXOM_SIGNER_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let http_state = std::sync::Arc::new((OpenBaoTransit::from_env()?, network.clone(), intents.as_ref().map(|i| i.mode()).unwrap_or("off")));
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/treasury", get(treasury))
        .with_state(http_state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tokio::spawn(async move { axum::serve(listener, app).await });

    let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
    let mut renewed: Option<std::time::Instant> = None;
    let mut price_cache = None;
    let mut last_treasury: Option<std::time::Instant> = None;
    loop {
        tick.tick().await;
        renew_token(&transit, &mut renewed).await;
        // The platform policy with the company's stricter rules, if any.
        let effective = axom_hr::company_policy::effective(&policy, &pool).await;
        let signer = Signer { pool: &pool, policy: &effective, cfg: &cfg, transit: &transit, near: &near, notify: &notify_cfg, intents: intents.as_deref() };
        let mut sent_any = false;
        match signer.process_due(chrono::Utc::now()).await {
            Ok(results) => {
                for (id, r) in results {
                    sent_any |= matches!(r, axom_signer::SendResult::Sent { .. });
                    tracing::info!(payout_id = %id, result = ?r, "payout processed");
                }
            }
            Err(e) => tracing::error!(error = %e, "signer cycle failed"),
        }
        if let Err(e) = signer.finalize_sent(chrono::Utc::now()).await {
            tracing::error!(error = %e, "finalize cycle failed");
        }
        // Treasury yield: at most once a minute, never in a tick that sent payouts (one nonce at a time).
        if !sent_any && last_treasury.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60)) {
            last_treasury = Some(std::time::Instant::now());
            let price = near_price(&mut price_cache, &network).await;
            log_treasury("", signer.manage_treasury(&near, price, chrono::Utc::now()).await);
        }
    }
}

async fn treasury(State(s): State<std::sync::Arc<(OpenBaoTransit, String, &'static str)>>) -> Json<serde_json::Value> {
    match s.0.public_key().await {
        Ok(pk) => Json(json!({ "network": s.1, "account": pk.implicit_account(), "public_key": pk.to_near_string(), "intents": s.2 })),
        Err(e) => Json(json!({ "network": s.1, "error": e.to_string(), "intents": s.2 })),
    }
}

/// Multi-tenant platform: `DATABASE_URL` is the platform database. Every 10
/// seconds, for each active company with a treasury, send its approved
/// payouts from its own treasury key and finalize sent ones. A company
/// without a treasury keeps its payouts waiting. One company's failure
/// never blocks another's.
/// Renew the signer's periodic OpenBao token once an hour, so it doesn't
/// expire a month after setup and stop every payout.
async fn renew_token(transit: &OpenBaoTransit, last: &mut Option<std::time::Instant>) {
    if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3600)) {
        return;
    }
    *last = Some(std::time::Instant::now());
    match transit.renew_self().await {
        Ok(()) => tracing::debug!("OpenBao token renewed"),
        // Sealed or down: payouts are paused anyway; try again next hour.
        Err(e @ (axom_signer::openbao::TransitError::Sealed | axom_signer::openbao::TransitError::Unavailable(_))) => tracing::warn!(error = %e, "could not renew the OpenBao token yet"),
        Err(e) => tracing::error!(
            error = %e,
            "the signer's OpenBao token can't renew itself and will expire: apply infra/openbao/axom-signer.hcl again (it grants auth/token/renew-self), or issue the signer a new token"
        ),
    }
}

async fn multi_tenant(cfg: SignerConfig, policy: PolicyConfig, rpc_url: String, intents: Option<Box<dyn IntentsApi>>) -> anyhow::Result<()> {
    use std::collections::HashMap;
    let base = OpenBaoTransit::from_env()?; // validates OPENBAO_ADDR and the token
    let addr = std::env::var("OPENBAO_ADDR")?;
    let token = axom_signer::openbao::token_from_env()?;
    let near = JsonRpcNear::new(rpc_url);
    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set (the platform database)")?;
    let opts = PgConnectOptions::from_str(&db_url)?.options([("search_path", "public")]);
    let platform = PgPoolOptions::new().max_connections(3).connect_with(opts.clone()).await?;
    let bind = std::env::var("AXOM_SIGNER_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let app = Router::new().route("/health", get(|| async { Json(json!({ "status": "ok", "mode": "multi-tenant" })) }));
    tokio::spawn(async move { axum::serve(listener, app).await });
    tracing::info!("signer running for all companies with a treasury");

    let mut pools: HashMap<String, sqlx::PgPool> = HashMap::new();
    let mut price_cache = None;
    let mut last_treasury: HashMap<String, std::time::Instant> = HashMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
    let mut renewed: Option<std::time::Instant> = None;
    loop {
        tick.tick().await;
        // One token signs for every company's treasury key.
        renew_token(&base, &mut renewed).await;
        let companies: Vec<(String, String, String, String)> = match sqlx::query_as(
            "SELECT slug, db_name, treasury_key, owner_email FROM platform.tenants WHERE status = 'active' AND treasury_key IS NOT NULL",
        )
        .fetch_all(&platform)
        .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "listing companies failed");
                continue;
            }
        };
        for (slug, db_name, key, owner_email) in companies {
            let pool = pools
                .entry(db_name.clone())
                .or_insert_with(|| PgPoolOptions::new().max_connections(2).idle_timeout(std::time::Duration::from_secs(600)).connect_lazy_with(opts.clone().database(&db_name)))
                .clone();
            let transit = OpenBaoTransit::new(&addr, &token, &key);
            let notify = NotifyConfig { owner_email, dashboard_url: std::env::var("AXOM_DASHBOARD_URL").unwrap_or_default() };
            let effective = axom_hr::company_policy::effective(&policy, &pool).await;
            let signer = Signer { pool: &pool, policy: &effective, cfg: &cfg, transit: &transit, near: &near, notify: &notify, intents: intents.as_deref() };
            let mut sent_any = false;
            match signer.process_due(chrono::Utc::now()).await {
                Ok(results) => {
                    for (id, r) in results {
                        sent_any |= matches!(r, axom_signer::SendResult::Sent { .. });
                        tracing::info!(company = %slug, payout_id = %id, result = ?r, "payout processed");
                    }
                }
                Err(e) => tracing::error!(company = %slug, error = %e, "signer cycle failed"),
            }
            if let Err(e) = signer.finalize_sent(chrono::Utc::now()).await {
                tracing::error!(company = %slug, error = %e, "finalize cycle failed");
            }
            let due = last_treasury.get(&slug).is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60));
            if !sent_any && due {
                last_treasury.insert(slug.clone(), std::time::Instant::now());
                let price = near_price(&mut price_cache, &cfg.network).await;
                log_treasury(&slug, signer.manage_treasury(&near, price, chrono::Utc::now()).await);
            }
        }
    }
}
