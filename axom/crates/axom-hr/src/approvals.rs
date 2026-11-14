//! Human approval of payouts. Only a named, active admin with an
//! MFA-verified session (from Supabase Auth, checked in axom-api) may approve
//! or reject. An approval produces a signed record the signer verifies.
//! Nothing here is reachable through MCP or any agent-callable function.

use axom_core::audit::{self, AuditEvent};
use axom_core::authz::{AuthorizationKey, AuthorizationKind, PayoutAuthorization};
use axom_core::db::{audit_invalid_transition, TransitionDbError};
use axom_core::domain::Actor;
use axom_core::state::PayoutStatus;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::payouts;
use crate::payroll::store_authorization_tx;
use crate::policy::PolicyConfig;

/// An authenticated dashboard session.
#[derive(Debug, Clone, Serialize)]
pub struct AdminSession {
    pub user_id: Uuid,
    pub email: String,
    pub session_id: String,
    /// True only for an `aal2` (TOTP-verified) Supabase session.
    pub mfa_verified: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval requires an MFA-verified session")]
    MfaRequired,
    #[error("not an active admin")]
    NotAdmin,
    #[error("payout has no NEAR amount (no price at creation); it cannot be approved")]
    NoAmount,
    #[error(transparent)]
    Payout(#[from] payouts::PayoutError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// How long a human approval authorizes sending.
pub const APPROVAL_TTL_DAYS: i64 = 7;

async fn admin_name(pool: &PgPool, s: &AdminSession) -> Result<String, ApprovalError> {
    let row: Option<(String, bool)> = sqlx::query_as("SELECT name, active FROM axom.admins WHERE user_id = $1")
        .bind(s.user_id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some((name, true)) => Ok(name),
        _ => Err(ApprovalError::NotAdmin),
    }
}

async fn deny(pool: &PgPool, s: &AdminSession, action: &str, payout_id: Uuid, err: &ApprovalError, cfg: &PolicyConfig) -> Result<(), sqlx::Error> {
    let actor = Actor::Admin { name: s.email.clone(), mfa_verified: s.mfa_verified };
    let ev = AuditEvent::new(&actor, action, "denied")
        .inputs(json!({ "payout_id": payout_id, "session_id": s.session_id, "user_id": s.user_id }))
        .reasoning(err.to_string())
        .policy_version(cfg.version());
    audit::append(pool, &ev).await.map(|_| ())
}

async fn authorize_session(pool: &PgPool, cfg: &PolicyConfig, s: &AdminSession, action: &str, payout_id: Uuid) -> Result<Actor, ApprovalError> {
    let check = async {
        if !s.mfa_verified {
            return Err(ApprovalError::MfaRequired);
        }
        let name = admin_name(pool, s).await?;
        Ok(Actor::Admin { name, mfa_verified: true })
    }
    .await;
    if let Err(e) = &check {
        deny(pool, s, action, payout_id, e, cfg).await?;
    }
    check
}

pub async fn approve_payout(
    pool: &PgPool,
    cfg: &PolicyConfig,
    approval_key: &AuthorizationKey,
    session: &AdminSession,
    payout_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Uuid, ApprovalError> {
    let actor = authorize_session(pool, cfg, session, "payout.approve", payout_id).await?;
    let Actor::Admin { name, .. } = &actor else { unreachable!() };

    let (recipient, yocto, usd, network, policy_version): (String, Option<String>, Decimal, String, String) = sqlx::query_as(
        "SELECT recipient, amount_yocto::text, amount_usd, network, policy_version FROM axom.payouts WHERE id = $1",
    )
    .bind(payout_id)
    .fetch_one(pool)
    .await?;
    let yocto = yocto.ok_or(ApprovalError::NoAmount)?;
    let destination = crate::payroll::payout_destination(pool, payout_id).await?;

    // MFA-verified admin, by policy (the same check `payouts::approve` makes).
    let decision = match payouts::require_admin(cfg, &actor, payout_id) {
        Ok(d) => d,
        Err(e) => {
            let ev = AuditEvent::new(&actor, "payout.approve", "denied").inputs(json!({ "payout_id": payout_id })).reasoning(e.to_string()).policy_version(cfg.version());
            audit::append(pool, &ev).await?;
            return Err(e.into());
        }
    };

    // The status change, the approval record and the signed authorization
    // commit together. Separately, a failure after the first step left the
    // payout approved with nothing the signer would accept, and no way to
    // approve it again.
    let approval_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    // State machine: only pending_approval -> approved. Never touches the cap.
    match payouts::move_payout_tx(&mut tx, &actor, payout_id, PayoutStatus::Approved, "approved by admin", &decision.policy_version, now).await {
        Ok(()) => {}
        Err(payouts::PayoutError::Transition(TransitionDbError::Invalid(err))) => {
            tx.rollback().await?;
            audit_invalid_transition(pool, &actor, "axom.payouts", payout_id, &err).await?;
            return Err(payouts::PayoutError::Transition(TransitionDbError::Invalid(err)).into());
        }
        Err(e) => return Err(e.into()),
    }
    sqlx::query(
        "INSERT INTO axom.approvals (id, payout_id, decision, admin_id, admin_name, session_id, mfa_verified, created_at)
         VALUES ($1,$2,'approve',$3,$4,$5,true,$6)",
    )
    .bind(approval_id)
    .bind(payout_id)
    .bind(session.user_id)
    .bind(name)
    .bind(&session.session_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let payload = PayoutAuthorization {
        kind: AuthorizationKind::HumanApproval,
        payout_id,
        network,
        recipient,
        amount_yocto: yocto,
        amount_usd: usd.normalize().to_string(),
        policy_version,
        approval_id: Some(approval_id),
        approver: Some(name.clone()),
        issued_at: now,
        expires_at: now + Duration::days(APPROVAL_TTL_DAYS),
        destination,
    };
    store_authorization_tx(&mut tx, approval_key.sign(payload), cfg).await?;
    tx.commit().await?;
    Ok(approval_id)
}

pub async fn reject_payout(
    pool: &PgPool,
    cfg: &PolicyConfig,
    session: &AdminSession,
    payout_id: Uuid,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<Uuid, ApprovalError> {
    let actor = authorize_session(pool, cfg, session, "payout.reject", payout_id).await?;
    let Actor::Admin { name, .. } = &actor else { unreachable!() };
    payouts::reject(pool, cfg, &actor, payout_id, reason).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.approvals (id, payout_id, decision, admin_id, admin_name, session_id, mfa_verified, reason, created_at)
         VALUES ($1,$2,'reject',$3,$4,$5,true,$6,$7)",
    )
    .bind(id)
    .bind(payout_id)
    .bind(session.user_id)
    .bind(name)
    .bind(&session.session_id)
    .bind(reason)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(id)
}
