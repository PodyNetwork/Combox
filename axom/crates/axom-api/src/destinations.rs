//! Cross-chain payout destinations (NEAR Intents).
//!
//! Workers (per-worker bearer token):
//! - `GET    /workers/me/payout-destination`: verified NEAR account, active destination, supported chains;
//! - `POST   /workers/me/payout-destination/challenge`: `{chain, asset_id, address}` -> message to sign;
//! - `POST   /workers/me/payout-destination/verify`: `{challenge_id, signature}` (hex ed25519 by the verified NEAR key);
//! - `DELETE /workers/me/payout-destination`: back to NEAR payouts.
//!
//! Before any of that, and before any payout at all, a worker proves they
//! hold the NEAR account they are paid at (per-worker bearer token):
//! - `POST /workers/me/payout-address/challenge`: `{account_id, public_key}` -> message to sign;
//! - `POST /workers/me/payout-address/verify`: `{challenge_id, signature}` (hex ed25519 over the message).
//!
//! Admins: `GET /payouts/cross-chain` lists recent cross-chain payouts with
//! their intent status and destination-chain transactions.

use axom_core::intents::{self, Destination, SUPPORTED_CHAINS};
use axom_hr::destinations::{self, DestinationErr};
use axom_hr::gates;
use axom_tools::near_rpc::{testnet_rpc_url, JsonRpcNear};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::intake::worker_from_headers;
use crate::{reply, ApiError, AppState};

fn network() -> String {
    intents::network_from_env()
}

fn dest_error(e: DestinationErr) -> Response {
    match e {
        DestinationErr::Db(e) => ApiError(e.to_string()).into_response(),
        DestinationErr::Rpc(_) => reply(StatusCode::BAD_GATEWAY, json!({ "error": e.to_string() })),
        other => reply(StatusCode::BAD_REQUEST, json!({ "error": other.to_string() })),
    }
}

fn rpc() -> JsonRpcNear {
    match network().as_str() {
        "mainnet" => JsonRpcNear::new(std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| "https://rpc.mainnet.fastnear.com".into())),
        _ => JsonRpcNear::new(testnet_rpc_url()),
    }
}

fn gate_error(e: gates::GateError) -> Response {
    use gates::GateError::*;
    match e {
        Db(e) => ApiError(e.to_string()).into_response(),
        Rpc(_) => reply(StatusCode::BAD_GATEWAY, json!({ "error": e.to_string() })),
        other => reply(StatusCode::BAD_REQUEST, json!({ "error": other.to_string() })),
    }
}

#[derive(Deserialize)]
pub struct AddressBody {
    pub account_id: String,
    /// The account's key, `ed25519:<base58>`.
    pub public_key: String,
}

/// Start verifying the NEAR account a worker is paid at: returns a message
/// to sign with that account's key. Without a verified address every payout
/// to the worker is blocked.
pub async fn address_challenge(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<AddressBody>) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    if b.account_id.len() > 64 || b.public_key.len() > 100 {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "that isn't a NEAR account and key" }));
    }
    match gates::issue_challenge(&s.pool, worker, &network(), b.account_id.trim(), b.public_key.trim(), Utc::now()).await {
        Ok(ch) => reply(StatusCode::CREATED, json!(ch)),
        Err(e) => gate_error(e),
    }
}

pub async fn address_verify(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<VerifyBody>) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    // The challenge must belong to the caller.
    let owner: Option<(Uuid,)> = match sqlx::query_as("SELECT worker_id FROM axom.address_challenges WHERE id = $1").bind(b.challenge_id).fetch_optional(&s.pool).await {
        Ok(o) => o,
        Err(e) => return ApiError(e.to_string()).into_response(),
    };
    if owner.map(|o| o.0) != Some(worker) {
        return gate_error(gates::GateError::BadChallenge(b.challenge_id));
    }
    // A named account's key is checked on chain (it must be a full-access key).
    let net = network();
    match gates::verify_challenge(&s.pool, &rpc(), &net, b.challenge_id, &b.signature, Utc::now()).await {
        Ok(_) => match gates::verified_address(&s.pool, worker, &net).await {
            Ok(near_account) => reply(StatusCode::OK, json!({ "network": net, "near_account": near_account })),
            Err(e) => ApiError(e.to_string()).into_response(),
        },
        Err(e) => gate_error(e),
    }
}

pub async fn get_mine(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    let net = network();
    let result = async {
        let near_account = gates::verified_address(&s.pool, worker, &net).await?;
        let active = destinations::active(&s.pool, worker, &net).await?;
        Ok::<_, sqlx::Error>((near_account, active))
    }
    .await;
    match result {
        Ok((near_account, active)) => reply(
            StatusCode::OK,
            json!({
                "network": net,
                "near_account": near_account,
                "destination": active,
                "paid_in": if active.is_some() { "cross_chain" } else { "near" },
                "chains": SUPPORTED_CHAINS.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
            }),
        ),
        Err(e) => ApiError(e.to_string()).into_response(),
    }
}

pub async fn challenge(State(s): State<AppState>, headers: HeaderMap, Json(d): Json<Destination>) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    match destinations::issue_challenge(&s.pool, worker, &network(), &d, Utc::now()).await {
        Ok(ch) => reply(StatusCode::CREATED, json!(ch)),
        Err(e) => dest_error(e),
    }
}

#[derive(Deserialize)]
pub struct VerifyBody {
    pub challenge_id: Uuid,
    pub signature: String,
}

pub async fn verify(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<VerifyBody>) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    // The challenge must belong to the caller.
    let owner: Option<(Uuid,)> = match sqlx::query_as("SELECT worker_id FROM axom.destination_challenges WHERE id = $1").bind(b.challenge_id).fetch_optional(&s.pool).await {
        Ok(o) => o,
        Err(e) => return ApiError(e.to_string()).into_response(),
    };
    if owner.map(|o| o.0) != Some(worker) {
        return dest_error(DestinationErr::BadChallenge(b.challenge_id));
    }
    // Named NEAR accounts are re-checked on chain (the key must still be full access).
    match destinations::verify_challenge(&s.pool, &rpc(), b.challenge_id, &b.signature, Utc::now()).await {
        Ok(id) => {
            let active = destinations::active(&s.pool, worker, &network()).await.ok().flatten();
            reply(StatusCode::OK, json!({ "destination_id": id, "destination": active }))
        }
        Err(e) => dest_error(e),
    }
}

pub async fn revoke(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    match destinations::revoke(&s.pool, worker, &network(), Utc::now()).await {
        Ok(revoked) => reply(StatusCode::OK, json!({ "revoked": revoked, "paid_in": "near" })),
        Err(e) => ApiError(e.to_string()).into_response(),
    }
}

type XRow = (
    Uuid,
    Option<Uuid>,
    String,
    rust_decimal::Decimal,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    chrono::DateTime<Utc>,
);

pub async fn list(State(s): State<AppState>, headers: HeaderMap) -> Response {
    // Recipients, amounts and destination addresses: MFA-verified admins of
    // this company only, like the pending payouts.
    if let Err(r) = crate::admin::mfa_admin(&s, &headers).await {
        return *r;
    }
    let rows: Result<Vec<XRow>, _> = sqlx::query_as(
        "SELECT id, worker_id, recipient, amount_usd, status, dest_chain, dest_asset, dest_address,
                tx_hash, intent_deposit_address, intent_status, intent_tx_hashes, intent_quote -> 'quote', created_at
         FROM axom.payouts WHERE destination_id IS NOT NULL ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(&s.pool)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    let q = r.12.unwrap_or_default();
                    json!({
                        "id": r.0, "worker_id": r.1, "near_account": r.2, "amount_usd": r.3, "status": r.4,
                        "destination": { "chain": r.5, "chain_name": intents::chain_name(&r.5), "asset_id": r.6, "address": r.7 },
                        "near_tx_hash": r.8, "deposit_address": r.9, "intent_status": r.10, "destination_txs": r.11.unwrap_or(json!([])),
                        "amount_out": q.get("amountOutFormatted"), "amount_out_usd": q.get("amountOutUsd"), "created_at": r.13,
                    })
                })
                .collect();
            reply(StatusCode::OK, json!({ "network": network(), "payouts": list }))
        }
        Err(e) => ApiError(e.to_string()).into_response(),
    }
}
