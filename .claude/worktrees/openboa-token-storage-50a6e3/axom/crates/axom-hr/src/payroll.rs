//! Payroll: compute what each worker is owed for a pay period, apply the
//! payout gates, create payouts (policy + cap), and attach the automatic
//! attestation to under-threshold payouts so the signer can send them.

use axom_core::audit::{self, AuditEvent};
use axom_core::authz::{AuthorizationKey, AuthorizationKind, PayoutAuthorization};
use axom_core::db;
use axom_core::domain::{Actor, PayUnit};
use axom_core::state::PayoutStatus;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::destinations;
use crate::gates::{self, GateBlock};
use crate::payouts::{self, IntentRoute, PayoutCreated, PayoutError, PayoutRequest};
use crate::policy::{PolicyConfig, PriceQuote};

/// What a worker is owed for a period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Owed {
    pub worker_id: Uuid,
    pub pay_unit: PayUnit,
    pub rate_usd: Decimal,
    /// Completed tasks (task pay) or hours on completed tasks (hourly pay).
    pub units: Decimal,
    pub amount_usd: Decimal,
}

/// Amount owed = completed work in `[start, end)` × pay rate.
pub async fn compute_owed(pool: &PgPool, worker_id: Uuid, start: DateTime<Utc>, end: DateTime<Utc>) -> anyhow::Result<Owed> {
    let w = db::load_worker(pool, worker_id).await?.ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found"))?;
    let (tasks, hours): (i64, Decimal) = sqlx::query_as(
        "SELECT count(*), COALESCE(SUM(hours_worked), 0) FROM axom.tasks
         WHERE assignee = $1 AND status = 'done' AND completed_at >= $2 AND completed_at < $3",
    )
    .bind(worker_id)
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await?;
    let units = match w.pay_unit {
        PayUnit::Task => Decimal::from(tasks),
        PayUnit::Hour => hours,
    };
    Ok(Owed { worker_id, pay_unit: w.pay_unit, rate_usd: w.pay_rate_usd, units, amount_usd: (units * w.pay_rate_usd).round_dp(2) })
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PayrollOutcome {
    NothingOwed,
    AlreadyPaid { payout_id: Uuid },
    Blocked { payout_id: Uuid, reasons: Vec<String> },
    Created { created: PayoutCreated, attested: bool },
    /// An earlier payout for this period never paid and no new one was
    /// created this run (still over the cap, or too many have failed).
    Waiting { payout_id: Uuid, reason: String },
}

/// Payouts created for one worker and pay period before payroll stops and
/// leaves it to the owner (each failed one has already alerted them).
pub const MAX_PAYOUTS_PER_PERIOD: usize = 3;

/// How far back payroll looks for completed work that was never paid.
pub const LOOKBACK_WEEKS: i64 = 8;

#[derive(Debug, Clone, Serialize)]
pub struct PayrollLine {
    pub owed: Owed,
    pub outcome: PayrollOutcome,
}

/// Inputs shared by one payroll run.
pub struct PayrollRun<'a> {
    pub policy: &'a PolicyConfig,
    pub price: Option<PriceQuote>,
    /// Estimated network fee per transfer, in NEAR (counts toward the cap).
    pub fee_near: Decimal,
    pub network: &'a str,
    pub attestor: &'a AuthorizationKey,
    pub period: (DateTime<Utc>, DateTime<Utc>),
}

pub async fn run_payroll(pool: &PgPool, run: &PayrollRun<'_>, now: DateTime<Utc>) -> anyhow::Result<Vec<PayrollLine>> {
    let (start, end) = run.period;
    let mut lines = Vec::new();
    for w in db::load_workers(pool).await? {
        let owed = compute_owed(pool, w.id, start, end).await?;
        if owed.amount_usd <= Decimal::ZERO {
            lines.push(PayrollLine { owed, outcome: PayrollOutcome::NothingOwed });
            continue;
        }
        // A payout that is paid, on its way, or was rejected holds the period.
        let existing: Option<(Uuid,)> = sqlx::query_as(&format!(
            "SELECT id FROM axom.payouts WHERE worker_id = $1 AND period_start = $2 AND {}",
            payouts::HOLDS_PERIOD
        ))
        .bind(w.id)
        .bind(start)
        .fetch_optional(pool)
        .await?;
        if let Some((payout_id,)) = existing {
            lines.push(PayrollLine { owed, outcome: PayrollOutcome::AlreadyPaid { payout_id } });
            continue;
        }
        // Earlier payouts for this period that never paid and never will
        // (over the cap, approval expired, abandoned after the last failed
        // attempt), newest first. The period is paid again with a new payout,
        // up to a limit.
        let unpaid: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, status FROM axom.payouts WHERE worker_id = $1 AND period_start = $2 AND status <> 'blocked' ORDER BY created_at DESC")
                .bind(w.id)
                .bind(start)
                .fetch_all(pool)
                .await?;
        if unpaid.len() >= MAX_PAYOUTS_PER_PERIOD {
            let reason = format!("{} payouts for this pay period never paid; it needs the owner", unpaid.len());
            lines.push(PayrollLine { owed, outcome: PayrollOutcome::Waiting { payout_id: unpaid[0].0, reason } });
            continue;
        }

        let recipient = match gates::check_payout_gates(pool, w.id, run.network).await? {
            Ok(addr) => addr,
            Err(blocks) => {
                let payout_id = record_blocked(pool, run, &owed, &blocks, now).await?;
                lines.push(PayrollLine {
                    owed,
                    outcome: PayrollOutcome::Blocked { payout_id, reasons: blocks.iter().map(|b| b.reason().to_string()).collect() },
                });
                continue;
            }
        };

        // Cross-chain: only a destination signed by the key of the NEAR
        // address being paid. Otherwise the worker is paid in NEAR.
        let route = match destinations::active(pool, w.id, run.network).await? {
            Some(d) if d.near_account == recipient => {
                let bps = Decimal::from(run.policy.intents().route_fee_bps) / Decimal::from(10_000);
                Some(IntentRoute { destination_id: d.id, destination: d.destination, route_fee_usd: (owed.amount_usd * bps).round_dp(6) })
            }
            Some(d) => {
                tracing::warn!(worker_id = %w.id, signed_by = %d.near_account, %recipient, "cross-chain destination signed by another NEAR address; paying in NEAR");
                None
            }
            None => None,
        };
        // A cross-chain deposit also pays two wNEAR storage registrations.
        let fee_near = match route {
            Some(_) => run.fee_near + Decimal::from_i128_with_scale(axom_core::intents::DEPOSIT_OVERHEAD_YOCTO as i128, 24),
            None => run.fee_near,
        };
        let network_fee_usd = run.price.map(|q| (fee_near * q.usd_per_near).round_dp(6)).unwrap_or(Decimal::ZERO);
        let fee_usd = network_fee_usd + route.as_ref().map(|r| r.route_fee_usd).unwrap_or(Decimal::ZERO);
        // Over the cap last time: wait until it fits (the cap was raised or
        // a new month began) rather than record, and alert on, another one
        // every run.
        if let Some((payout_id, _)) = unpaid.first().filter(|(_, status)| status == PayoutStatus::CapExceeded.as_str()) {
            let fits = match crate::cap::usage(pool, crate::cap::month_of(now)).await {
                Ok(u) => u.remaining_usd() >= owed.amount_usd + fee_usd,
                Err(_) => true,
            };
            if !fits {
                lines.push(PayrollLine { owed, outcome: PayrollOutcome::Waiting { payout_id: *payout_id, reason: "still over the monthly cap".into() } });
                continue;
            }
        }
        let req = PayoutRequest {
            worker_id: Some(w.id),
            recipient,
            amount_usd: owed.amount_usd,
            fee_usd,
            network: run.network.to_string(),
            period: Some(run.period),
            route,
        };
        let created = match payouts::create_payout(pool, run.policy, &Actor::System, &req, run.price, now).await {
            Ok(c) => c,
            Err(PayoutError::Denied(reason)) => {
                tracing::warn!(worker_id = %w.id, %reason, "payout denied by policy");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let attested = if created.status == PayoutStatus::Approved {
            issue_auto_attestation(pool, run.policy, run.attestor, created.payout_id, now).await?;
            true
        } else {
            false
        };
        lines.push(PayrollLine { owed, outcome: PayrollOutcome::Created { created, attested } });
    }
    Ok(lines)
}

async fn record_blocked(pool: &PgPool, run: &PayrollRun<'_>, owed: &Owed, blocks: &[GateBlock], now: DateTime<Utc>) -> Result<Uuid, sqlx::Error> {
    let reason = blocks.iter().map(|b| b.reason()).collect::<Vec<_>>().join("; ");
    // Re-runs update the existing blocked record instead of adding rows.
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM axom.payouts WHERE worker_id = $1 AND period_start = $2 AND status = 'blocked' ORDER BY created_at LIMIT 1")
            .bind(owed.worker_id)
            .bind(run.period.0)
            .fetch_optional(pool)
            .await?;
    if let Some((id,)) = existing {
        sqlx::query("UPDATE axom.payouts SET status_reason = $2, amount_usd = $3, updated_at = $4 WHERE id = $1")
            .bind(id)
            .bind(&reason)
            .bind(owed.amount_usd)
            .bind(now)
            .execute(pool)
            .await?;
        return Ok(id);
    }
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO axom.payouts (id, worker_id, recipient, amount_usd, fee_usd, status, status_reason, policy_version,
                                   network, period_start, period_end, created_at, updated_at)
         VALUES ($1,$2,'',$3,0,'blocked',$4,$5,$6,$7,$8,$9,$9)",
    )
    .bind(id)
    .bind(owed.worker_id)
    .bind(owed.amount_usd)
    .bind(&reason)
    .bind(run.policy.version())
    .bind(run.network)
    .bind(run.period.0)
    .bind(run.period.1)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let ev = AuditEvent::new(&Actor::System, "payout.create", "blocked")
        .inputs(json!({ "payout_id": id, "worker_id": owed.worker_id, "amount_usd": owed.amount_usd, "gates": blocks }))
        .reasoning(&reason)
        .policy_version(run.policy.version())
        .at(now);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(id)
}

/// How long an automatic attestation stays valid.
pub const AUTO_ATTESTATION_TTL_HOURS: i64 = 24;

/// Sign and store the automatic under-threshold attestation for a payout the
/// policy engine approved on its own (status approved, no human approval).
pub async fn issue_auto_attestation(pool: &PgPool, cfg: &PolicyConfig, key: &AuthorizationKey, payout_id: Uuid, now: DateTime<Utc>) -> anyhow::Result<()> {
    let row: (String, String, Option<String>, Decimal, String, String, i64) = sqlx::query_as(
        "SELECT p.status, p.recipient, p.amount_yocto::text, p.amount_usd, p.network, p.policy_version,
                (SELECT count(*) FROM axom.approvals a WHERE a.payout_id = p.id)
         FROM axom.payouts p WHERE p.id = $1",
    )
    .bind(payout_id)
    .fetch_one(pool)
    .await?;
    let (status, recipient, yocto, usd, network, policy_version, approvals) = row;
    let destination = payout_destination(pool, payout_id).await?;
    anyhow::ensure!(status == "approved", "payout {payout_id} is {status}; only automatically approved payouts get an attestation");
    anyhow::ensure!(approvals == 0, "payout {payout_id} was approved by a human; it carries that approval instead");
    let yocto = yocto.ok_or_else(|| anyhow::anyhow!("payout {payout_id} has no NEAR amount"))?;
    let payload = PayoutAuthorization {
        kind: AuthorizationKind::AutoAttestation,
        payout_id,
        network,
        recipient,
        amount_yocto: yocto,
        amount_usd: usd.normalize().to_string(),
        policy_version,
        approval_id: None,
        approver: None,
        issued_at: now,
        expires_at: now + Duration::hours(AUTO_ATTESTATION_TTL_HOURS),
        destination,
    };
    store_authorization(pool, key.sign(payload), cfg).await
}

/// The cross-chain destination recorded on a payout, if any.
pub async fn payout_destination(pool: &PgPool, payout_id: Uuid) -> Result<Option<axom_core::intents::Destination>, sqlx::Error> {
    let row: (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT dest_chain, dest_asset, dest_address FROM axom.payouts WHERE id = $1").bind(payout_id).fetch_one(pool).await?;
    Ok(match row {
        (Some(chain), Some(asset_id), Some(address)) => Some(axom_core::intents::Destination { chain, asset_id, address }),
        _ => None,
    })
}

pub(crate) async fn store_authorization(pool: &PgPool, signed: axom_core::authz::SignedAuthorization, cfg: &PolicyConfig) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    store_authorization_tx(&mut tx, signed, cfg).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn store_authorization_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    signed: axom_core::authz::SignedAuthorization,
    cfg: &PolicyConfig,
) -> anyhow::Result<()> {
    let n = sqlx::query(
        "INSERT INTO axom.payout_authorizations (id, payout_id, kind, payload, signature) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (payout_id) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(signed.payload.payout_id)
    .bind(signed.payload.kind.as_str())
    .bind(serde_json::to_value(&signed.payload)?)
    .bind(&signed.signature)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    anyhow::ensure!(n == 1, "payout {} already has an authorization", signed.payload.payout_id);
    let ev = AuditEvent::new(&Actor::System, "payout.authorize", signed.payload.kind.as_str())
        .inputs(json!({ "payout_id": signed.payload.payout_id, "approval_id": signed.payload.approval_id, "expires_at": signed.payload.expires_at }))
        .policy_version(cfg.version());
    audit::append(&mut **tx, &ev).await?;
    Ok(())
}

/// What [`refresh_authorizations`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Refreshed {
    /// Approved payouts that had no authorization and were given one.
    pub issued: Vec<Uuid>,
    /// Automatic attestations that had expired and were signed again.
    pub renewed: Vec<Uuid>,
    /// Payouts given up because they could no longer be authorized.
    pub abandoned: Vec<Uuid>,
}

/// Keep approved payouts sendable, or give them up cleanly. Run before each
/// payroll. Without it an approved payout whose authorization is missing or
/// expired stays approved forever: the signer refuses it on every cycle, its
/// cap reservation is held, and its pay period can't be paid again.
///
/// - No authorization, approved automatically: the attestation is issued (a
///   crash between creating the payout and attesting it).
/// - Automatic attestation expired (the vault was sealed, the signer down or
///   the kill switch on for a day): signed again, while the payout is younger
///   than the approval expiry. That is the same staleness a human approval
///   allows; the signer still re-runs the policy as of the payout's creation.
/// - Anything else that can't be authorized any more (older than that, an
///   expired human approval, a human approval with no authorization): given
///   up, if nothing was signed for it. Payroll then pays the period with a
///   new payout at the current price, which needs approval again if it did
///   before.
pub async fn refresh_authorizations(pool: &PgPool, cfg: &PolicyConfig, key: &AuthorizationKey, now: DateTime<Utc>) -> anyhow::Result<Refreshed> {
    let rows: Vec<(Uuid, DateTime<Utc>, Option<serde_json::Value>, i64)> = sqlx::query_as(
        "SELECT p.id, p.created_at, a.payload, (SELECT count(*) FROM axom.approvals ap WHERE ap.payout_id = p.id)
         FROM axom.payouts p LEFT JOIN axom.payout_authorizations a ON a.payout_id = p.id
         WHERE p.status = 'approved' ORDER BY p.created_at",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Refreshed::default();
    for (id, created_at, payload, approvals) in rows {
        let fresh_enough = now - created_at < cfg.approval_expiry();
        let give_up = match payload {
            None if approvals == 0 && fresh_enough => {
                issue_auto_attestation(pool, cfg, key, id, now).await?;
                out.issued.push(id);
                None
            }
            None if approvals == 0 => Some("approved automatically, but never attested and now too old to attest"),
            None => Some("approved by an admin, but its authorization was never recorded"),
            Some(payload) => {
                let Ok(mut auth) = serde_json::from_value::<PayoutAuthorization>(payload) else { continue };
                if auth.expires_at > now {
                    continue;
                }
                match auth.kind {
                    AuthorizationKind::AutoAttestation if fresh_enough => {
                        auth.issued_at = now;
                        auth.expires_at = now + Duration::hours(AUTO_ATTESTATION_TTL_HOURS);
                        let signed = key.sign(auth);
                        let mut tx = pool.begin().await?;
                        sqlx::query("UPDATE axom.payout_authorizations SET payload = $2, signature = $3 WHERE payout_id = $1 AND kind = 'auto_attestation'")
                            .bind(id)
                            .bind(serde_json::to_value(&signed.payload)?)
                            .bind(&signed.signature)
                            .execute(&mut *tx)
                            .await?;
                        let ev = AuditEvent::new(&Actor::System, "payout.authorize", "renewed")
                            .inputs(json!({ "payout_id": id, "expires_at": signed.payload.expires_at }))
                            .reasoning("the automatic attestation expired before the payout could be sent")
                            .policy_version(cfg.version())
                            .at(now);
                        audit::append(&mut *tx, &ev).await?;
                        tx.commit().await?;
                        out.renewed.push(id);
                        None
                    }
                    AuthorizationKind::AutoAttestation => Some("its automatic attestation expired and it is too old to attest again"),
                    AuthorizationKind::HumanApproval => Some("its approval expired before it could be sent"),
                }
            }
        };
        if let Some(why) = give_up {
            let reason = format!("{why}; the pay period will be paid with a new payout");
            if payouts::abandon_unsent(pool, cfg, id, &reason, now).await? {
                out.abandoned.push(id);
            }
        }
    }
    Ok(out)
}

/// The pay periods a payroll run covers: every complete week in the last
/// [`LOOKBACK_WEEKS`] in which a freelancer finished work, oldest first, and
/// always the most recent complete week. A week is not paid only in the week
/// that follows it: a tax form or address that arrives later, a week the
/// runtime was down or had no price, or a payout that failed for good would
/// otherwise leave that work unpaid forever. Weeks already paid cost one
/// query per worker.
pub async fn periods_to_run(pool: &PgPool, now: DateTime<Utc>) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>, sqlx::Error> {
    let latest = last_complete_week(now);
    let weeks: Vec<(chrono::NaiveDateTime,)> = sqlx::query_as(
        "SELECT DISTINCT date_trunc('week', t.completed_at AT TIME ZONE 'UTC')
         FROM axom.tasks t JOIN axom.workers w ON w.id = t.assignee
         WHERE t.status = 'done' AND w.kind = 'human_freelancer' AND t.completed_at >= $1 AND t.completed_at < $2
         ORDER BY 1",
    )
    .bind(latest.1 - Duration::weeks(LOOKBACK_WEEKS))
    .bind(latest.1)
    .fetch_all(pool)
    .await?;
    let mut periods: Vec<_> = weeks.into_iter().map(|(start,)| (start.and_utc(), start.and_utc() + Duration::days(7))).collect();
    if !periods.contains(&latest) {
        periods.push(latest);
    }
    Ok(periods)
}

/// The time a payroll run is evaluated at, given the price quote it uses:
/// never earlier than the quote. The policy (and the signer, which re-runs it
/// as of the payout's creation) requires approval for a quote stamped after
/// the decision time, so a run must not be dated before its own quote.
pub fn clock_after_quote(quote: &PriceQuote, now: DateTime<Utc>) -> DateTime<Utc> {
    now.max(quote.fetched_at)
}

/// The most recent complete weekly pay period (Monday 00:00 UTC to Monday 00:00 UTC).
pub fn last_complete_week(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    use chrono::Datelike;
    let today = now.date_naive();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let end = monday.and_hms_opt(0, 0, 0).expect("midnight").and_utc();
    (end - Duration::days(7), end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn weekly_period() {
        // Sunday 2026-11-08 -> week of Mon 2026-10-26 .. Mon 2026-11-02.
        let (s, e) = last_complete_week(Utc.with_ymd_and_hms(2026, 11, 8, 15, 0, 0).unwrap());
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap());
        // Monday 00:00 itself closes the previous week.
        let (s, _) = last_complete_week(Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0).unwrap());
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap());
    }
}
