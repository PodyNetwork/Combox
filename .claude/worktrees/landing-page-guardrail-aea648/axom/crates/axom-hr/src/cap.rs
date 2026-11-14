//! Monthly payout cap ledger.
//!
//! One shared cap for all worker payouts per UTC calendar month. Every payout
//! reserves its USD value (amount plus network fee) when it is created,
//! including while pending approval. Reservations are later committed (sent)
//! or released (rejected, expired). All checks run inside a transaction that
//! holds a `FOR UPDATE` lock on the month row, so concurrent payouts
//! serialize and cannot jointly exceed the cap.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::policy::{self, Action, Context, PolicyConfig};

#[derive(Debug, thiserror::Error)]
pub enum CapError {
    #[error("monthly cap exceeded: {used} used + {requested} requested > {cap} cap for {month}")]
    Exceeded { month: NaiveDate, cap: Decimal, used: Decimal, requested: Decimal },
    #[error("plan budget {budget} USD exceeds the monthly cap of {cap} USD")]
    PlanBudgetExceedsCap { budget: Decimal, cap: Decimal },
    #[error("cap change denied: {0}")]
    ChangeDenied(String),
    #[error("cap is not configured")]
    NotSeeded,
    #[error("payout {0} has no open reservation")]
    NoOpenReservation(Uuid),
    #[error("invalid amount: {0}")]
    InvalidAmount(Decimal),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// First day of the UTC calendar month containing `at`.
pub fn month_of(at: DateTime<Utc>) -> NaiveDate {
    NaiveDate::from_ymd_opt(at.year(), at.month(), 1).expect("valid month start")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapUsage {
    pub month: NaiveDate,
    pub cap_usd: Decimal,
    /// Reservations not yet committed or released (includes pending approvals).
    pub reserved_usd: Decimal,
    pub committed_usd: Decimal,
}

impl CapUsage {
    pub fn used_usd(&self) -> Decimal {
        self.reserved_usd + self.committed_usd
    }
    pub fn remaining_usd(&self) -> Decimal {
        (self.cap_usd - self.used_usd()).max(Decimal::ZERO)
    }
}

/// Insert the cap row from config if no cap exists yet. Never overwrites.
pub async fn seed_cap(pool: &PgPool, cfg: &PolicyConfig) -> Result<Decimal, CapError> {
    sqlx::query(
        "INSERT INTO axom.cap_settings (id, cap_usd, updated_by) VALUES (1, $1, 'seed:config')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(cfg.file.cap.default_monthly_cap_usd)
    .execute(pool)
    .await?;
    current_cap(pool).await
}

pub async fn current_cap<'e, E: PgExecutor<'e>>(ex: E) -> Result<Decimal, CapError> {
    let row: Option<(Decimal,)> = sqlx::query_as("SELECT cap_usd FROM axom.cap_settings WHERE id = 1")
        .fetch_optional(ex)
        .await?;
    row.map(|r| r.0).ok_or(CapError::NotSeeded)
}

/// Sum ledger entries for a month: (reserved_open, committed).
async fn sums<'e, E: PgExecutor<'e>>(ex: E, month: NaiveDate) -> Result<(Decimal, Decimal), sqlx::Error> {
    let (reserved, committed, released): (Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT
            COALESCE(SUM(amount_usd) FILTER (WHERE kind = 'reservation'), 0),
            COALESCE(SUM(amount_usd) FILTER (WHERE kind = 'commit'), 0),
            COALESCE(SUM(amount_usd) FILTER (WHERE kind = 'release'), 0)
         FROM axom.cap_ledger WHERE month = $1",
    )
    .bind(month)
    .fetch_one(ex)
    .await?;
    // Each payout's reservation ends as either a commit or a release of the
    // same amount, so open reservations = reserved - committed - released.
    Ok((reserved - committed - released, committed))
}

pub async fn usage<'e, E: PgExecutor<'e> + Copy>(ex: E, month: NaiveDate) -> Result<CapUsage, CapError> {
    let cap = current_cap(ex).await?;
    let (reserved, committed) = sums(ex, month).await?;
    Ok(CapUsage { month, cap_usd: cap, reserved_usd: reserved, committed_usd: committed })
}

/// Lock the month row, creating it if needed. Also locks the cap row so a
/// concurrent cap change serializes with reservations.
async fn lock_month(tx: &mut Transaction<'_, Postgres>, month: NaiveDate) -> Result<Decimal, CapError> {
    sqlx::query("INSERT INTO axom.cap_months (month) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(month)
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT month FROM axom.cap_months WHERE month = $1 FOR UPDATE")
        .bind(month)
        .execute(&mut **tx)
        .await?;
    let row: Option<(Decimal,)> = sqlx::query_as("SELECT cap_usd FROM axom.cap_settings WHERE id = 1 FOR SHARE")
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|r| r.0).ok_or(CapError::NotSeeded)
}

/// Reserve `amount` for an existing payout row in the month of `now`.
/// Returns the reservation (ledger entry) id, or `CapError::Exceeded`.
pub async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    payout_id: Uuid,
    amount: Decimal,
    now: DateTime<Utc>,
) -> Result<Uuid, CapError> {
    if amount < Decimal::ZERO {
        return Err(CapError::InvalidAmount(amount));
    }
    let month = month_of(now);
    let cap = lock_month(tx, month).await?;
    let (reserved, committed) = sums(&mut **tx, month).await?;
    let used = reserved + committed;
    if used + amount > cap {
        return Err(CapError::Exceeded { month, cap, used, requested: amount });
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.cap_ledger (id, payout_id, month, kind, amount_usd, created_at)
         VALUES ($1, $2, $3, 'reservation', $4, $5)",
    )
    .bind(id)
    .bind(payout_id)
    .bind(month)
    .bind(amount)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// The open reservation for a payout: (month, amount).
async fn open_reservation(tx: &mut Transaction<'_, Postgres>, payout_id: Uuid) -> Result<(NaiveDate, Decimal), CapError> {
    let row: Option<(NaiveDate, Decimal, bool)> = sqlx::query_as(
        "SELECT r.month, r.amount_usd,
                EXISTS (SELECT 1 FROM axom.cap_ledger e
                        WHERE e.payout_id = r.payout_id AND e.kind IN ('commit','release'))
         FROM axom.cap_ledger r WHERE r.payout_id = $1 AND r.kind = 'reservation'",
    )
    .bind(payout_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some((month, amount, false)) => Ok((month, amount)),
        _ => Err(CapError::NoOpenReservation(payout_id)),
    }
}

async fn close(tx: &mut Transaction<'_, Postgres>, payout_id: Uuid, kind: &str, now: DateTime<Utc>) -> Result<Uuid, CapError> {
    let (month, amount) = open_reservation(tx, payout_id).await?;
    lock_month(tx, month).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.cap_ledger (id, payout_id, month, kind, amount_usd, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(payout_id)
    .bind(month)
    .bind(kind)
    .bind(amount)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Commit a payout's reservation (payout sent). Counts in the reservation's month.
pub async fn commit(tx: &mut Transaction<'_, Postgres>, payout_id: Uuid, now: DateTime<Utc>) -> Result<Uuid, CapError> {
    close(tx, payout_id, "commit", now).await
}

/// Release a payout's reservation (rejected or expired).
pub async fn release(tx: &mut Transaction<'_, Postgres>, payout_id: Uuid, now: DateTime<Utc>) -> Result<Uuid, CapError> {
    close(tx, payout_id, "release", now).await
}

/// Reject a plan whose budget exceeds the cap. Plans cannot raise the cap.
pub async fn validate_plan_budget<'e, E: PgExecutor<'e>>(ex: E, budget: Decimal) -> Result<(), CapError> {
    let cap = current_cap(ex).await?;
    check_plan_budget(budget, cap)
}

pub fn check_plan_budget(budget: Decimal, cap: Decimal) -> Result<(), CapError> {
    if budget > cap {
        Err(CapError::PlanBudgetExceedsCap { budget, cap })
    } else {
        Ok(())
    }
}

/// Change the cap. Only an MFA-verified admin may do this; every attempt is
/// audited, including denied ones.
pub async fn change_cap(
    pool: &PgPool,
    cfg: &PolicyConfig,
    actor: &Actor,
    new_cap: Decimal,
    reason: &str,
) -> Result<(), CapError> {
    let action = Action::ChangeCap { new_cap_usd: new_cap };
    let decision = policy::evaluate(cfg, &action, &Context::new(actor.clone(), Utc::now()));
    if !decision.is_allowed() || new_cap < Decimal::ZERO {
        let why = if new_cap < Decimal::ZERO { "cap cannot be negative".to_string() } else { decision.reason.clone() };
        let ev = AuditEvent::new(actor, "cap.change", "denied")
            .inputs(json!({ "new_cap_usd": new_cap, "reason": reason }))
            .reasoning(&why)
            .policy_version(&decision.policy_version);
        audit::append(pool, &ev).await?;
        return Err(CapError::ChangeDenied(why));
    }
    let Actor::Admin { name, .. } = actor else { unreachable!("policy allows only admins") };
    let mut tx = pool.begin().await?;
    let old: Option<(Decimal,)> = sqlx::query_as("SELECT cap_usd FROM axom.cap_settings WHERE id = 1 FOR UPDATE")
        .fetch_optional(&mut *tx)
        .await?;
    let ev = AuditEvent::new(actor, "cap.change", "applied")
        .inputs(json!({ "old_cap_usd": old.map(|o| o.0), "new_cap_usd": new_cap, "reason": reason }))
        .reasoning(&decision.reason)
        .policy_version(&decision.policy_version);
    let audit_id = audit::append(&mut *tx, &ev).await?;
    sqlx::query(
        "INSERT INTO axom.cap_settings (id, cap_usd, updated_by, updated_at) VALUES (1, $1, $2, now())
         ON CONFLICT (id) DO UPDATE SET cap_usd = EXCLUDED.cap_usd, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(new_cap)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO axom.cap_changes (id, old_cap_usd, new_cap_usd, admin, audit_id) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(old.map(|o| o.0))
    .bind(new_cap)
    .bind(name)
    .bind(audit_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn month_boundaries_are_utc() {
        let last = Utc.with_ymd_and_hms(2026, 11, 30, 23, 59, 59).unwrap();
        let first = Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap();
        assert_eq!(month_of(last), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(month_of(first), NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
    }

    #[test]
    fn usage_math() {
        let u = CapUsage {
            month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            cap_usd: dec!(500),
            reserved_usd: dec!(120),
            committed_usd: dec!(300.5),
        };
        assert_eq!(u.used_usd(), dec!(420.5));
        assert_eq!(u.remaining_usd(), dec!(79.5));
    }

    #[test]
    fn plan_budget_check() {
        assert!(check_plan_budget(dec!(500), dec!(500)).is_ok());
        assert!(matches!(check_plan_budget(dec!(500.01), dec!(500)), Err(CapError::PlanBudgetExceedsCap { .. })));
    }
}
