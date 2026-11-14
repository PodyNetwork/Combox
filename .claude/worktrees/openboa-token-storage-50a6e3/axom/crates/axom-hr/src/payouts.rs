//! Payout creation and lifecycle, combining the policy engine with the cap
//! ledger. Signing and sending arrive in milestone 4; this module owns the
//! records, statuses, and cap reservations.

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{audit_invalid_transition, transition_in_tx, TransitionDbError};
use axom_core::domain::{parse_db, Actor};
use axom_core::intents::Destination;
use axom_core::near;
use axom_core::state::{PayoutStatus, StateMachine};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::cap::{self, CapError};
use crate::notify;
use crate::policy::{self, Action, Context, Decision, Outcome, PolicyConfig, PriceQuote};

#[derive(Debug, thiserror::Error)]
pub enum PayoutError {
    #[error("payout denied by policy: {0}")]
    Denied(String),
    #[error("not authorized: {0}")]
    Unauthorized(String),
    #[error(transparent)]
    Transition(#[from] TransitionDbError),
    #[error(transparent)]
    Cap(#[from] CapError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct PayoutRequest {
    pub worker_id: Option<Uuid>,
    pub recipient: String,
    pub amount_usd: Decimal,
    pub fee_usd: Decimal,
    /// `testnet` unless NEAR_NETWORK=mainnet is set explicitly.
    pub network: String,
    /// Pay period this payout covers, if it comes from payroll.
    pub period: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// Pay on another chain through NEAR Intents instead of in NEAR to
    /// `recipient` (which stays the worker's verified NEAR account).
    pub route: Option<IntentRoute>,
}

/// A cross-chain route for one payout.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentRoute {
    /// The worker's active, NEAR-signed destination.
    pub destination_id: Uuid,
    pub destination: Destination,
    /// Estimated route cost, added to the NEAR deposit (and included in `fee_usd`).
    pub route_fee_usd: Decimal,
}

impl Default for PayoutRequest {
    fn default() -> Self {
        Self {
            worker_id: None,
            recipient: String::new(),
            amount_usd: Decimal::ZERO,
            fee_usd: Decimal::ZERO,
            network: "testnet".into(),
            period: None,
            route: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PayoutCreated {
    pub payout_id: Uuid,
    pub status: PayoutStatus,
    pub reservation_id: Option<Uuid>,
    pub decision: Decision,
}

/// Statuses that count toward the per-recipient rolling window.
const WINDOW_STATUSES: &[&str] = &["pending_approval", "approved", "sent", "finalized", "failed"];

pub async fn kill_switch_on(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as("SELECT kill_switch FROM axom.system_controls WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    // Fail closed: a missing controls row behaves like an engaged kill switch.
    Ok(row.map(|r| r.0).unwrap_or(true))
}

/// Create a payout: evaluate policy, reserve the cap, and record the result.
///
/// - deny: no payout row is created; the denial is audited.
/// - allow: status `approved` (automatic under-threshold attestation).
/// - require_approval: status `pending_approval`.
/// - cap would be exceeded: status `cap_exceeded`, owner notified, nothing reserved.
pub async fn create_payout(
    pool: &PgPool,
    cfg: &PolicyConfig,
    actor: &Actor,
    req: &PayoutRequest,
    price: Option<PriceQuote>,
    now: DateTime<Utc>,
) -> Result<PayoutCreated, PayoutError> {
    let mut tx = pool.begin().await?;

    // Serialize payouts to the same recipient so the rolling-window sum is exact.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("payout-recipient:{}", req.recipient))
        .execute(&mut *tx)
        .await?;

    let window_start = now - cfg.aggregation_window();
    let (window_sum,): (Decimal,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_usd), 0) FROM axom.payouts
         WHERE recipient = $1 AND created_at > $2 AND created_at <= $3 AND status = ANY($4)",
    )
    .bind(&req.recipient)
    .bind(window_start)
    .bind(now)
    .bind(WINDOW_STATUSES)
    .fetch_one(&mut *tx)
    .await?;

    let kill: Option<(bool,)> = sqlx::query_as("SELECT kill_switch FROM axom.system_controls WHERE id = 1")
        .fetch_optional(&mut *tx)
        .await?;

    let action = Action::Payout { recipient: req.recipient.clone(), amount_usd: req.amount_usd, fee_usd: req.fee_usd };
    let ctx = Context {
        actor: actor.clone(),
        now,
        kill_switch: kill.map(|k| k.0).unwrap_or(true),
        price,
        recipient_window_usd: window_sum,
    };
    let decision = policy::evaluate(cfg, &action, &ctx);
    let inputs = json!({
        "worker_id": req.worker_id, "recipient": req.recipient,
        "amount_usd": req.amount_usd, "fee_usd": req.fee_usd,
        "recipient_window_usd": window_sum, "price": price,
        "destination": req.route.as_ref().map(|r| &r.destination),
        "route_fee_usd": req.route.as_ref().map(|r| r.route_fee_usd),
    });

    if decision.outcome == Outcome::Deny {
        tx.rollback().await?;
        let ev = AuditEvent::new(actor, "payout.create", "denied")
            .inputs(inputs)
            .reasoning(&decision.reason)
            .policy_version(&decision.policy_version)
            .at(now);
        audit::append(pool, &ev).await?;
        return Err(PayoutError::Denied(decision.reason));
    }

    let intended = if decision.outcome == Outcome::Allow { PayoutStatus::Approved } else { PayoutStatus::PendingApproval };
    // The NEAR amount is fixed at creation from the quote used for the
    // decision (a stale quote still sets the amount, but forces approval).
    // A cross-chain payout deposits the route fee on top, so the worker
    // receives what they are owed after the swap.
    let route_fee = req.route.as_ref().map(|r| r.route_fee_usd).unwrap_or(Decimal::ZERO);
    let amount_yocto = match price {
        Some(q) => Some(near::usd_to_yocto(req.amount_usd + route_fee, q.usd_per_near).map_err(|e| PayoutError::Denied(e.to_string()))?.to_string()),
        None => None,
    };
    let payout_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.payouts (id, worker_id, recipient, amount_usd, fee_usd, status, status_reason,
                                   policy_version, created_at, updated_at,
                                   network, amount_yocto, usd_per_near, price_fetched_at, period_start, period_end,
                                   destination_id, dest_chain, dest_asset, dest_address)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,$10,$11::numeric,$12,$13,$14,$15,$16,$17,$18,$19)",
    )
    .bind(payout_id)
    .bind(req.worker_id)
    .bind(&req.recipient)
    .bind(req.amount_usd)
    .bind(req.fee_usd)
    .bind(PayoutStatus::start(intended).map_err(TransitionDbError::from)?.as_str())
    .bind(&decision.reason)
    .bind(&decision.policy_version)
    .bind(now)
    .bind(&req.network)
    .bind(amount_yocto)
    .bind(price.map(|q| q.usd_per_near))
    .bind(price.map(|q| q.fetched_at))
    .bind(req.period.map(|p| p.0))
    .bind(req.period.map(|p| p.1))
    .bind(req.route.as_ref().map(|r| r.destination_id))
    .bind(req.route.as_ref().map(|r| r.destination.chain.clone()))
    .bind(req.route.as_ref().map(|r| r.destination.asset_id.clone()))
    .bind(req.route.as_ref().map(|r| r.destination.address.clone()))
    .execute(&mut *tx)
    .await?;

    // Network fees (and cross-chain route fees) count toward the cap.
    let cap_amount = req.amount_usd + req.fee_usd;
    match cap::reserve(&mut tx, payout_id, cap_amount, now).await {
        Ok(reservation_id) => {
            sqlx::query("UPDATE axom.payouts SET reservation_id = $2 WHERE id = $1")
                .bind(payout_id)
                .bind(reservation_id)
                .execute(&mut *tx)
                .await?;
            let ev = AuditEvent::new(actor, "payout.create", intended.as_str())
                .inputs(json!({ "payout_id": payout_id, "reservation_id": reservation_id, "request": inputs }))
                .reasoning(&decision.reason)
                .policy_version(&decision.policy_version)
                .at(now);
            audit::append(&mut *tx, &ev).await?;
            tx.commit().await?;
            Ok(PayoutCreated { payout_id, status: intended, reservation_id: Some(reservation_id), decision })
        }
        Err(CapError::Exceeded { month, cap, used, requested }) => {
            let reason = format!("monthly cap exceeded: {used} used + {requested} requested > {cap} cap for {month}");
            sqlx::query("UPDATE axom.payouts SET status = 'cap_exceeded', status_reason = $2 WHERE id = $1")
                .bind(payout_id)
                .bind(&reason)
                .execute(&mut *tx)
                .await?;
            let ev = AuditEvent::new(actor, "payout.create", "cap_exceeded")
                .inputs(json!({ "payout_id": payout_id, "month": month, "cap_usd": cap, "used_usd": used, "requested_usd": requested, "request": inputs }))
                .reasoning(&reason)
                .policy_version(&decision.policy_version)
                .at(now);
            let audit_id = audit::append(&mut *tx, &ev).await?;
            notify::enqueue(
                &mut *tx,
                "cap_exceeded",
                &format!("Payout blocked: monthly cap of {cap} USD reached"),
                json!({ "payout_id": payout_id, "recipient": req.recipient, "requested_usd": requested, "used_usd": used, "cap_usd": cap, "month": month }),
                Some(audit_id),
            )
            .await?;
            tx.commit().await?;
            let decision = Decision { outcome: decision.outcome, reason, policy_version: decision.policy_version, notify_owner: true };
            Ok(PayoutCreated { payout_id, status: PayoutStatus::CapExceeded, reservation_id: None, decision })
        }
        Err(e) => Err(e.into()),
    }
}

/// Which payouts hold a worker's pay period (SQL over `axom.payouts`). One
/// that never paid and never will (blocked by a gate, over the cap, approval
/// expired, abandoned after its last attempt) does not, so the period can be
/// paid once the cause is gone. A rejected payout does: an admin decided.
/// The unique index `payouts_worker_period_idx` uses the same condition.
pub const HOLDS_PERIOD: &str = "status NOT IN ('blocked','cap_exceeded','expired') AND abandoned_at IS NULL";

/// Move a payout to a new status, applying cap side effects in the same
/// transaction: rejected/expired release the reservation; sent commits it.
/// Used by the signer for sent/failed/finalized.
pub async fn move_payout(
    pool: &PgPool,
    actor: &Actor,
    payout_id: Uuid,
    to: PayoutStatus,
    reason: &str,
    policy_version: &str,
    now: DateTime<Utc>,
) -> Result<(), PayoutError> {
    let mut tx = pool.begin().await?;
    match move_payout_tx(&mut tx, actor, payout_id, to, reason, policy_version, now).await {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(PayoutError::Transition(TransitionDbError::Invalid(err))) => {
            tx.rollback().await?;
            audit_invalid_transition(pool, actor, "axom.payouts", payout_id, &err).await?;
            Err(TransitionDbError::Invalid(err).into())
        }
        Err(e) => Err(e),
    }
}

/// [`move_payout`] inside the caller's transaction, so a status change and
/// whatever must go with it (an approval record, a signed authorization)
/// commit together or not at all. On an invalid transition the caller rolls
/// back and records it with [`audit_invalid_transition`].
pub async fn move_payout_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &Actor,
    payout_id: Uuid,
    to: PayoutStatus,
    reason: &str,
    policy_version: &str,
    now: DateTime<Utc>,
) -> Result<(), PayoutError> {
    let from = transition_in_tx::<PayoutStatus>(tx, payout_id, to).await?;
    if to.releases_reservation() {
        cap::release(tx, payout_id, now).await?;
    } else if to == PayoutStatus::Sent {
        // From approved, or from failed on a retry: the reservation is still open.
        cap::commit(tx, payout_id, now).await?;
    }
    sqlx::query("UPDATE axom.payouts SET status_reason = $2, updated_at = $3 WHERE id = $1")
        .bind(payout_id)
        .bind(reason)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    let ev = AuditEvent::new(actor, "payout.transition", to.as_str())
        .inputs(json!({ "payout_id": payout_id, "from": from.as_str(), "to": to.as_str() }))
        .reasoning(reason)
        .policy_version(policy_version)
        .at(now);
    audit::append(&mut **tx, &ev).await?;
    Ok(())
}

pub(crate) fn require_admin(cfg: &PolicyConfig, actor: &Actor, payout_id: Uuid) -> Result<Decision, PayoutError> {
    let d = policy::evaluate(cfg, &Action::ApprovePayout { payout_id }, &Context::new(actor.clone(), Utc::now()));
    if d.is_allowed() {
        Ok(d)
    } else {
        Err(PayoutError::Unauthorized(d.reason))
    }
}

/// Approve a pending payout. MFA-verified admin only. Approval never touches
/// the cap: the reservation was made at creation, and a `cap_exceeded` payout
/// cannot be approved (the state machine forbids it).
pub async fn approve(pool: &PgPool, cfg: &PolicyConfig, actor: &Actor, payout_id: Uuid) -> Result<(), PayoutError> {
    let d = match require_admin(cfg, actor, payout_id) {
        Ok(d) => d,
        Err(e) => {
            let ev = AuditEvent::new(actor, "payout.approve", "denied")
                .inputs(json!({ "payout_id": payout_id }))
                .reasoning(e.to_string())
                .policy_version(cfg.version());
            audit::append(pool, &ev).await?;
            return Err(e);
        }
    };
    move_payout(pool, actor, payout_id, PayoutStatus::Approved, "approved by admin", &d.policy_version, Utc::now()).await
}

/// Reject a pending payout and release its reservation. MFA-verified admin only.
pub async fn reject(pool: &PgPool, cfg: &PolicyConfig, actor: &Actor, payout_id: Uuid, reason: &str) -> Result<(), PayoutError> {
    let d = require_admin(cfg, actor, payout_id)?;
    move_payout(pool, actor, payout_id, PayoutStatus::Rejected, reason, &d.policy_version, Utc::now()).await
}

/// Expire pending approvals older than the configured expiry and release
/// their reservations. Returns the expired payout ids.
pub async fn expire_pending(pool: &PgPool, cfg: &PolicyConfig, now: DateTime<Utc>) -> Result<Vec<Uuid>, PayoutError> {
    let cutoff = now - cfg.approval_expiry();
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM axom.payouts WHERE status = 'pending_approval' AND created_at <= $1 ORDER BY created_at",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (id,) in ids {
        move_payout(pool, &Actor::System, id, PayoutStatus::Expired, "approval expired", cfg.version(), now).await?;
        out.push(id);
    }
    Ok(out)
}

/// Record that an approved payout was sent; commits its reservation.
/// (The signer that actually sends arrives in milestone 4.)
pub async fn mark_sent(pool: &PgPool, cfg: &PolicyConfig, payout_id: Uuid, now: DateTime<Utc>) -> Result<(), PayoutError> {
    move_payout(pool, &Actor::System, payout_id, PayoutStatus::Sent, "sent by signer", cfg.version(), now).await
}

pub async fn status(pool: &PgPool, payout_id: Uuid) -> Result<PayoutStatus, PayoutError> {
    let (s,): (String,) = sqlx::query_as("SELECT status FROM axom.payouts WHERE id = $1")
        .bind(payout_id)
        .fetch_one(pool)
        .await?;
    Ok(parse_db(&s)?)
}

/// After the final retry: release the reservation of a failed payout (no
/// money moved) and leave it failed. Idempotent.
pub async fn release_failed(pool: &PgPool, cfg: &PolicyConfig, payout_id: Uuid, reason: &str, now: DateTime<Utc>) -> Result<(), PayoutError> {
    let mut tx = pool.begin().await?;
    let (status,): (String,) = sqlx::query_as("SELECT status FROM axom.payouts WHERE id = $1 FOR UPDATE").bind(payout_id).fetch_one(&mut *tx).await?;
    if status != PayoutStatus::Failed.as_str() {
        return Err(PayoutError::Denied(format!("payout {payout_id} is {status}, not failed")));
    }
    match cap::release(&mut tx, payout_id, now).await {
        Ok(_) | Err(CapError::NoOpenReservation(_)) => {}
        Err(e) => return Err(e.into()),
    }
    // Given up for good: it no longer holds its pay period, so payroll can
    // pay that period with a new payout.
    sqlx::query("UPDATE axom.payouts SET abandoned_at = COALESCE(abandoned_at, $2), next_attempt_at = NULL WHERE id = $1")
        .bind(payout_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let ev = AuditEvent::new(&Actor::System, "payout.final_failure", "released")
        .inputs(json!({ "payout_id": payout_id }))
        .reasoning(reason)
        .policy_version(cfg.version())
        .at(now);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(())
}

/// Give up on an approved payout that can no longer be sent (its
/// authorization expired) and that nothing was ever signed for: mark it
/// failed, release its cap reservation, and free its pay period, in one
/// transaction. Returns false (and changes nothing) if the payout has moved
/// on or a transaction was already signed for it; the signer settles those.
pub async fn abandon_unsent(pool: &PgPool, cfg: &PolicyConfig, payout_id: Uuid, reason: &str, now: DateTime<Utc>) -> Result<bool, PayoutError> {
    let mut tx = pool.begin().await?;
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, tx_hash FROM axom.payouts WHERE id = $1 FOR UPDATE").bind(payout_id).fetch_optional(&mut *tx).await?;
    match row {
        Some((status, None)) if status == PayoutStatus::Approved.as_str() => {}
        _ => return Ok(false),
    }
    move_payout_tx(&mut tx, &Actor::System, payout_id, PayoutStatus::Failed, reason, cfg.version(), now).await?;
    match cap::release(&mut tx, payout_id, now).await {
        Ok(_) | Err(CapError::NoOpenReservation(_)) => {}
        Err(e) => return Err(e.into()),
    }
    sqlx::query("UPDATE axom.payouts SET abandoned_at = $2, next_attempt_at = NULL, last_error = $3 WHERE id = $1")
        .bind(payout_id)
        .bind(now)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    let ev = AuditEvent::new(&Actor::System, "payout.final_failure", "released")
        .inputs(json!({ "payout_id": payout_id }))
        .reasoning(reason)
        .policy_version(cfg.version())
        .at(now);
    let audit_id = audit::append(&mut *tx, &ev).await?;
    notify::enqueue(&mut *tx, "payout_failed", "Payout not sent: its authorization expired", json!({ "payout_id": payout_id, "reason": reason }), Some(audit_id)).await?;
    tx.commit().await?;
    Ok(true)
}
