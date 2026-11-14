//! Cross-chain payout destinations (NEAR Intents).
//!
//! NEAR is the worker's identity: a worker first verifies a NEAR address by
//! challenge signature (see [`crate::gates`]). To be paid on another chain,
//! the worker then signs a second challenge naming the chain, the 1Click
//! asset, and the address, with that same verified NEAR key. Axom never
//! takes a destination from anyone else: not the agent, not an admin, not an
//! email. Revoking the destination returns the worker to NEAR payouts.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::intents::{self, Destination, DestinationError};
use axom_core::near::{self, PublicKey};
use axom_tools::near_rpc::NearRpc;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::gates::CHALLENGE_TTL_MINUTES;

#[derive(Debug, thiserror::Error)]
pub enum DestinationErr {
    #[error(transparent)]
    Invalid(#[from] DestinationError),
    #[error("verify a NEAR address first: the destination is signed with that address's key")]
    NoVerifiedAddress,
    #[error("challenge {0} not found, expired, or already used")]
    BadChallenge(Uuid),
    #[error("signature does not verify against the worker's verified NEAR key")]
    BadSignature,
    #[error("the signing key is no longer a full-access key on {0}")]
    KeyNotOnAccount(String),
    #[error("could not check the account on NEAR: {0}")]
    Rpc(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct DestinationChallenge {
    pub id: Uuid,
    /// Sign these exact UTF-8 bytes with the verified NEAR key (ed25519).
    pub message: String,
    pub near_account: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveDestination {
    pub id: Uuid,
    pub near_account: String,
    pub destination: Destination,
    pub verified_at: DateTime<Utc>,
}

/// The worker's verified NEAR account and key on `network`.
async fn verified_key(pool: &PgPool, worker_id: Uuid, network: &str) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT account_id, public_key FROM axom.worker_addresses
         WHERE worker_id = $1 AND network = $2 AND revoked_at IS NULL ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(worker_id)
    .bind(network)
    .fetch_optional(pool)
    .await
}

/// Issue a challenge naming `dest`, to be signed with the worker's verified NEAR key.
pub async fn issue_challenge(pool: &PgPool, worker_id: Uuid, network: &str, dest: &Destination, now: DateTime<Utc>) -> Result<DestinationChallenge, DestinationErr> {
    intents::validate_destination(dest)?;
    let (near_account, public_key) = verified_key(pool, worker_id, network).await?.ok_or(DestinationErr::NoVerifiedAddress)?;
    let id = Uuid::new_v4();
    let nonce = Uuid::new_v4().simple().to_string();
    let expires_at = now + Duration::minutes(CHALLENGE_TTL_MINUTES);
    let message = intents::destination_message(network, worker_id, &near_account, dest, &nonce, &expires_at.to_rfc3339());
    sqlx::query(
        "INSERT INTO axom.destination_challenges (id, worker_id, network, near_account, public_key, chain, asset_id, address, message, expires_at, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(worker_id)
    .bind(network)
    .bind(&near_account)
    .bind(&public_key)
    .bind(&dest.chain)
    .bind(&dest.asset_id)
    .bind(&dest.address)
    .bind(&message)
    .bind(expires_at)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(DestinationChallenge { id, message, near_account, expires_at })
}

/// Verify a signed destination challenge and make it the worker's active
/// destination on that network (replacing any previous one).
pub async fn verify_challenge(pool: &PgPool, near_rpc: &dyn NearRpc, challenge_id: Uuid, signature_hex: &str, now: DateTime<Utc>) -> Result<Uuid, DestinationErr> {
    type Row = (Uuid, String, String, String, String, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT worker_id, network, near_account, public_key, chain, asset_id, address, message, expires_at, used_at
         FROM axom.destination_challenges WHERE id = $1",
    )
    .bind(challenge_id)
    .fetch_optional(pool)
    .await?;
    let Some((worker_id, network, near_account, public_key, chain, asset_id, address, message, expires_at, used_at)) = row else {
        return Err(DestinationErr::BadChallenge(challenge_id));
    };
    if used_at.is_some() || now > expires_at {
        return Err(DestinationErr::BadChallenge(challenge_id));
    }
    // The NEAR address must still be verified with the same key.
    match verified_key(pool, worker_id, &network).await? {
        Some((a, k)) if a == near_account && k == public_key => {}
        _ => return Err(DestinationErr::NoVerifiedAddress),
    }
    let key = PublicKey::parse(&public_key).map_err(|_| DestinationErr::BadSignature)?;
    let sig: [u8; 64] = hex::decode(signature_hex.trim()).ok().and_then(|b| b.try_into().ok()).ok_or(DestinationErr::BadSignature)?;
    key.verify(message.as_bytes(), &sig).map_err(|_| DestinationErr::BadSignature)?;
    if !near::is_implicit_account(&near_account) {
        match near_rpc.view_access_key(&near_account, &key).await {
            Ok(Some(info)) if info.full_access => {}
            Ok(_) => return Err(DestinationErr::KeyNotOnAccount(near_account)),
            Err(e) => return Err(DestinationErr::Rpc(e.to_string())),
        }
    }
    let dest = Destination { chain, asset_id, address };
    intents::validate_destination(&dest)?;

    let mut tx = pool.begin().await?;
    let used = sqlx::query("UPDATE axom.destination_challenges SET used_at = $2 WHERE id = $1 AND used_at IS NULL")
        .bind(challenge_id)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if used != 1 {
        return Err(DestinationErr::BadChallenge(challenge_id));
    }
    sqlx::query("UPDATE axom.payout_destinations SET revoked_at = $3 WHERE worker_id = $1 AND network = $2 AND revoked_at IS NULL")
        .bind(worker_id)
        .bind(&network)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.payout_destinations (id, worker_id, network, near_account, chain, asset_id, address, challenge_id, signature, verified_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(id)
    .bind(worker_id)
    .bind(&network)
    .bind(&near_account)
    .bind(&dest.chain)
    .bind(&dest.asset_id)
    .bind(&dest.address)
    .bind(challenge_id)
    .bind(signature_hex.trim())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let ev = AuditEvent::new(&Actor::System, "destination.verify", "verified")
        .inputs(json!({ "worker_id": worker_id, "network": network, "near_account": near_account, "destination": dest, "challenge_id": challenge_id }))
        .reasoning("worker signed the cross-chain destination with their verified NEAR key")
        .at(now);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(id)
}

/// Stop paying cross-chain; later payouts go to the verified NEAR address.
pub async fn revoke(pool: &PgPool, worker_id: Uuid, network: &str, now: DateTime<Utc>) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("UPDATE axom.payout_destinations SET revoked_at = $3 WHERE worker_id = $1 AND network = $2 AND revoked_at IS NULL")
        .bind(worker_id)
        .bind(network)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        let ev = AuditEvent::new(&Actor::System, "destination.revoke", "revoked").inputs(json!({ "worker_id": worker_id, "network": network })).at(now);
        audit::append(&mut *tx, &ev).await?;
    }
    tx.commit().await?;
    Ok(n > 0)
}

pub async fn active(pool: &PgPool, worker_id: Uuid, network: &str) -> Result<Option<ActiveDestination>, sqlx::Error> {
    let row: Option<(Uuid, String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, near_account, chain, asset_id, address, verified_at FROM axom.payout_destinations
         WHERE worker_id = $1 AND network = $2 AND revoked_at IS NULL",
    )
    .bind(worker_id)
    .bind(network)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| ActiveDestination { id: r.0, near_account: r.1, destination: Destination { chain: r.2, asset_id: r.3, address: r.4 }, verified_at: r.5 }))
}

/// Whether `destination_id` is still the worker's active destination, with
/// exactly these fields, signed by the key of `near_account`.
pub async fn is_active(pool: &PgPool, worker_id: Uuid, network: &str, destination_id: Uuid, near_account: &str, d: &Destination) -> Result<bool, sqlx::Error> {
    Ok(match active(pool, worker_id, network).await? {
        Some(a) => a.id == destination_id && a.near_account == near_account && &a.destination == d,
        None => false,
    })
}
