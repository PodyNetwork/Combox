//! Gates before a worker's payouts: a NEAR address verified by challenge
//! signature, a tax form (W-9 or W-8BEN, collected via DocuSeal) on file,
//! and not hired through Upwork (those are paid through Upwork).
//! Workers bring their own addresses; ForeMan never holds worker keys.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::near::{self, PublicKey};
use axom_tools::near_rpc::NearRpc;
use axom_tools::{ContractService, TaxFormKind};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error("invalid address: {0}")]
    BadAddress(String),
    #[error("challenge {0} not found, expired, or already used")]
    BadChallenge(Uuid),
    #[error("signature does not verify against the claimed key")]
    BadSignature,
    #[error("the key is not a full-access key on {0}")]
    KeyNotOnAccount(String),
    #[error("could not check the account on NEAR: {0}")]
    Rpc(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct Challenge {
    pub id: Uuid,
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

pub const CHALLENGE_TTL_MINUTES: i64 = 30;

/// Issue a challenge the worker signs with the key for `account_id`.
pub async fn issue_challenge(
    pool: &PgPool,
    worker_id: Uuid,
    network: &str,
    account_id: &str,
    public_key: &str,
    now: DateTime<Utc>,
) -> Result<Challenge, GateError> {
    near::validate_account_id(account_id).map_err(|e| GateError::BadAddress(e.to_string()))?;
    let key = PublicKey::parse(public_key).map_err(|e| GateError::BadAddress(e.to_string()))?;
    if near::is_implicit_account(account_id) && key.implicit_account() != account_id {
        return Err(GateError::BadAddress("implicit account does not match the public key".into()));
    }
    let id = Uuid::new_v4();
    let nonce = Uuid::new_v4().simple();
    let expires_at = now + Duration::minutes(CHALLENGE_TTL_MINUTES);
    let message = format!(
        "ForeMan address verification\nnetwork: {network}\naccount: {account_id}\nworker: {worker_id}\nnonce: {nonce}\nexpires: {}",
        expires_at.to_rfc3339()
    );
    sqlx::query(
        "INSERT INTO axom.address_challenges (id, worker_id, account_id, public_key, message, expires_at, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(worker_id)
    .bind(account_id)
    .bind(key.to_near_string())
    .bind(&message)
    .bind(expires_at)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Challenge { id, message, expires_at })
}

/// Verify a signed challenge and record the address as verified.
///
/// The signature is ed25519 over the UTF-8 challenge message. For an implicit
/// account the key is the account. For a named account the key must also be
/// a full-access key on that account, checked on NEAR.
pub async fn verify_challenge(
    pool: &PgPool,
    near_rpc: &dyn NearRpc,
    network: &str,
    challenge_id: Uuid,
    signature_hex: &str,
    now: DateTime<Utc>,
) -> Result<Uuid, GateError> {
    type ChallengeRow = (Uuid, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>);
    let row: Option<ChallengeRow> = sqlx::query_as(
        "SELECT worker_id, account_id, public_key, message, expires_at, used_at FROM axom.address_challenges WHERE id = $1",
    )
    .bind(challenge_id)
    .fetch_optional(pool)
    .await?;
    let Some((worker_id, account_id, public_key, message, expires_at, used_at)) = row else {
        return Err(GateError::BadChallenge(challenge_id));
    };
    if used_at.is_some() || now > expires_at {
        return Err(GateError::BadChallenge(challenge_id));
    }
    let key = PublicKey::parse(&public_key).map_err(|e| GateError::BadAddress(e.to_string()))?;
    let sig: [u8; 64] = hex::decode(signature_hex.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(GateError::BadSignature)?;
    key.verify(message.as_bytes(), &sig).map_err(|_| GateError::BadSignature)?;

    if !near::is_implicit_account(&account_id) {
        match near_rpc.view_access_key(&account_id, &key).await {
            Ok(Some(info)) if info.full_access => {}
            Ok(_) => return Err(GateError::KeyNotOnAccount(account_id)),
            Err(e) => return Err(GateError::Rpc(e.to_string())),
        }
    }

    let mut tx = pool.begin().await?;
    let used = sqlx::query("UPDATE axom.address_challenges SET used_at = $2 WHERE id = $1 AND used_at IS NULL")
        .bind(challenge_id)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if used != 1 {
        return Err(GateError::BadChallenge(challenge_id));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.worker_addresses (id, worker_id, network, account_id, public_key, challenge_id, verified_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (worker_id, network, account_id) DO UPDATE
           SET public_key = EXCLUDED.public_key, challenge_id = EXCLUDED.challenge_id,
               verified_at = EXCLUDED.verified_at, revoked_at = NULL",
    )
    .bind(id)
    .bind(worker_id)
    .bind(network)
    .bind(&account_id)
    .bind(key.to_near_string())
    .bind(challenge_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE axom.workers SET wallet_address = $2 WHERE id = $1").bind(worker_id).bind(&account_id).execute(&mut *tx).await?;
    let ev = AuditEvent::new(&Actor::System, "address.verify", "verified")
        .inputs(json!({ "worker_id": worker_id, "network": network, "account_id": account_id, "public_key": key.to_near_string(), "challenge_id": challenge_id }))
        .at(now);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(id)
}

/// Whether `account_id` is a verified, unrevoked address of the worker.
pub async fn is_verified_address(pool: &PgPool, worker_id: Uuid, network: &str, account_id: &str) -> Result<bool, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM axom.worker_addresses
         WHERE worker_id = $1 AND network = $2 AND account_id = $3 AND revoked_at IS NULL",
    )
    .bind(worker_id)
    .bind(network)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn verified_address(pool: &PgPool, worker_id: Uuid, network: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT account_id FROM axom.worker_addresses
         WHERE worker_id = $1 AND network = $2 AND revoked_at IS NULL ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(worker_id)
    .bind(network)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

/// Record a tax form received through DocuSeal.
pub async fn record_tax_form(pool: &PgPool, worker_id: Uuid, kind: TaxFormKind, submission_id: Option<&str>) -> Result<(), sqlx::Error> {
    let kind = match kind {
        TaxFormKind::W9 => "w9",
        TaxFormKind::W8Ben => "w8ben",
    };
    sqlx::query(
        "INSERT INTO axom.tax_forms (id, worker_id, kind, docuseal_submission_id) VALUES ($1,$2,$3,$4)
         ON CONFLICT (worker_id, kind) DO UPDATE SET docuseal_submission_id = EXCLUDED.docuseal_submission_id, received_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(worker_id)
    .bind(kind)
    .bind(submission_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Pull the worker's tax form status from DocuSeal and record it if present.
pub async fn sync_tax_form(pool: &PgPool, contracts: &dyn ContractService, worker_id: Uuid, email: &str) -> anyhow::Result<bool> {
    match contracts.fetch_tax_form(email).await? {
        Some(kind) => {
            record_tax_form(pool, worker_id, kind, None).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub async fn has_tax_form(pool: &PgPool, worker_id: Uuid) -> Result<bool, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.tax_forms WHERE worker_id = $1").bind(worker_id).fetch_one(pool).await?;
    Ok(n > 0)
}

/// Why a payout is blocked, if it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum GateBlock {
    AddressUnverified,
    TaxFormMissing,
    /// Hired through Upwork: Upwork's terms forbid paying off-platform
    /// (without a conversion fee), so ForeMan never pays them in NEAR.
    HiredViaUpwork,
}

impl GateBlock {
    pub fn reason(&self) -> &'static str {
        match self {
            GateBlock::AddressUnverified => "blocked: worker has no verified NEAR address (challenge signature required)",
            GateBlock::TaxFormMissing => "blocked: no W-9 or W-8BEN on file",
            GateBlock::HiredViaUpwork => "blocked: hired through Upwork, so paid through Upwork, not in NEAR (Upwork terms forbid off-platform payment)",
        }
    }
}

/// Whether the worker was hired from an Upwork posting.
pub async fn hired_via_upwork(pool: &PgPool, worker_id: Uuid) -> Result<bool, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM axom.candidates c JOIN axom.job_postings j ON j.id = c.job_posting_id
         WHERE c.worker_id = $1 AND j.marketplace = 'upwork'",
    )
    .bind(worker_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

/// Check the gates. Returns the worker's verified address when they pass.
pub async fn check_payout_gates(pool: &PgPool, worker_id: Uuid, network: &str) -> Result<Result<String, Vec<GateBlock>>, sqlx::Error> {
    let mut blocks = Vec::new();
    let addr = verified_address(pool, worker_id, network).await?;
    if addr.is_none() {
        blocks.push(GateBlock::AddressUnverified);
    }
    if !has_tax_form(pool, worker_id).await? {
        blocks.push(GateBlock::TaxFormMissing);
    }
    if hired_via_upwork(pool, worker_id).await? {
        blocks.push(GateBlock::HiredViaUpwork);
    }
    Ok(match (addr, blocks.is_empty()) {
        (Some(a), true) => Ok(a),
        _ => Err(blocks),
    })
}

/// Pull tax forms from DocuSeal for active freelancers who have none on
/// file yet (matched by the email they applied with). Returns how many were
/// recorded.
pub async fn sync_missing_tax_forms(pool: &PgPool, contracts: &dyn ContractService) -> anyhow::Result<usize> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT w.id, c.email FROM axom.workers w JOIN axom.candidates c ON c.worker_id = w.id
         WHERE w.kind = 'human_freelancer' AND w.status IN ('active','on_probation') AND c.email IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM axom.tax_forms t WHERE t.worker_id = w.id)",
    )
    .fetch_all(pool)
    .await?;
    let mut n = 0;
    for (worker, email) in rows {
        if sync_tax_form(pool, contracts, worker, &email).await? {
            n += 1;
        }
    }
    Ok(n)
}
