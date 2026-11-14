//! Performance reviews: scheduling, metric computation from task records,
//! storage, probation, spot checks, and the hand-off to firing.

use axom_core::audit::{self, AuditEvent};
use axom_core::db;
use axom_core::domain::Actor;
use axom_core::state::WorkerStatus;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::firing::{self, FireReport};
use crate::notify::NotifyConfig;
use crate::performance::{self, Assessment, FiringInputs, Metrics, PerformanceConfig};
use crate::policy::PolicyConfig;

#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    #[error("task {0} not found or has no assignee")]
    NoTask(Uuid),
    #[error("LLM judge {0} is the same model as the worker being scored")]
    JudgeNotIndependent(String),
    #[error("quality score must be within 0..=1")]
    OutOfRange,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Record an objective quality check (tests passing / acceptance criteria).
pub async fn record_objective_check(pool: &PgPool, task_id: Uuid, passed: bool) -> Result<(), QualityError> {
    let n = sqlx::query("UPDATE axom.tasks SET objective_check_passed = $2 WHERE id = $1")
        .bind(task_id)
        .bind(passed)
        .execute(pool)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(QualityError::NoTask(task_id));
    }
    Ok(())
}

/// Record an LLM-judged quality score. The judge must be a different model
/// from the worker being scored.
pub async fn record_llm_quality(pool: &PgPool, task_id: Uuid, judge_model: &str, score: f64) -> Result<(), QualityError> {
    if !(0.0..=1.0).contains(&score) {
        return Err(QualityError::OutOfRange);
    }
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT w.model FROM axom.tasks t JOIN axom.workers w ON w.id = t.assignee WHERE t.id = $1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    let Some((worker_model,)) = row else { return Err(QualityError::NoTask(task_id)) };
    if worker_model.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(judge_model.trim())) {
        return Err(QualityError::JudgeNotIndependent(judge_model.to_string()));
    }
    sqlx::query("UPDATE axom.tasks SET llm_quality_score = $2, llm_judge_model = $3 WHERE id = $1")
        .bind(task_id)
        .bind(score)
        .bind(judge_model)
        .execute(pool)
        .await?;
    Ok(())
}

/// Metrics for a worker over tasks finished in `(start, end]`
/// (`start = None` means from the beginning).
pub async fn metrics<'e, E: PgExecutor<'e>>(ex: E, worker_id: Uuid, start: Option<DateTime<Utc>>, end: DateTime<Utc>) -> Result<Metrics, sqlx::Error> {
    let row: (i64, i64, i64, Decimal, i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT
            count(*) FILTER (WHERE status = 'done'),
            count(*) FILTER (WHERE status = 'failed'),
            count(*) FILTER (WHERE status = 'done' AND (deadline IS NULL OR (finished_at AT TIME ZONE 'UTC')::date <= deadline)),
            COALESCE(SUM(cost_usd), 0),
            count(objective_check_passed),
            count(*) FILTER (WHERE objective_check_passed),
            avg(llm_quality_score)
         FROM axom.tasks
         WHERE assignee = $1 AND status IN ('done','failed') AND finished_at IS NOT NULL
           AND ($2::timestamptz IS NULL OR finished_at > $2) AND finished_at <= $3",
    )
    .bind(worker_id)
    .bind(start)
    .bind(end)
    .fetch_one(ex)
    .await?;
    Ok(Metrics {
        completed: row.0,
        failed: row.1,
        on_time: row.2,
        total_cost_usd: row.3,
        objective_checks: row.4,
        objective_passed: row.5,
        llm_quality: row.6,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    TaskCount,
    PeriodEnd,
    Final,
}

impl Trigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Trigger::TaskCount => "task_count",
            Trigger::PeriodEnd => "period_end",
            Trigger::Final => "final",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredReview {
    pub id: Uuid,
    pub worker_id: Uuid,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub trigger: Trigger,
    pub metrics: Metrics,
    pub assessment: Assessment,
    pub spot_checked: bool,
}

impl StoredReview {
    /// Metrics as recorded in notifications and the audit log.
    pub fn metrics_json(&self) -> serde_json::Value {
        metrics_json(&self.metrics)
    }
}

pub fn metrics_json(m: &Metrics) -> serde_json::Value {
    json!({
        "completed": m.completed, "failed": m.failed,
        "completion_rate": m.completion_rate(), "on_time_rate": m.on_time_rate(),
        "cost_per_completed_usd": m.cost_per_completed_usd(), "objective_quality": m.objective_quality(),
        "llm_quality_informational": m.llm_quality,
    })
}

/// Start of the current review period: end of the last periodic review, or
/// when the worker was created.
async fn period_start(pool: &PgPool, worker_id: Uuid) -> Result<DateTime<Utc>, sqlx::Error> {
    let (start,): (DateTime<Utc>,) = sqlx::query_as(
        "SELECT COALESCE(
            (SELECT max(period_end) FROM axom.reviews WHERE worker_id = $1 AND trigger <> 'final'),
            (SELECT created_at FROM axom.workers WHERE id = $1))",
    )
    .bind(worker_id)
    .fetch_one(pool)
    .await?;
    Ok(start)
}

/// Whether a review is due, and why.
pub async fn review_due(pool: &PgPool, cfg: &PerformanceConfig, worker_id: Uuid, now: DateTime<Utc>) -> Result<Option<Trigger>, sqlx::Error> {
    let start = period_start(pool, worker_id).await?;
    let m = metrics(pool, worker_id, Some(start), now).await?;
    if m.finished() >= cfg.file.review.every_finished_tasks {
        Ok(Some(Trigger::TaskCount))
    } else if now - start >= cfg.period() {
        Ok(Some(Trigger::PeriodEnd))
    } else {
        Ok(None)
    }
}

/// Compute and store a review. Every review is stored and audited; about
/// `spot_check_rate` of them go to the owner's spot-check queue.
pub async fn store_review(
    pool: &PgPool,
    cfg: &PerformanceConfig,
    worker_id: Uuid,
    trigger: Trigger,
    now: DateTime<Utc>,
) -> Result<StoredReview, sqlx::Error> {
    let (start, window_start) = match trigger {
        Trigger::Final => {
            let (created,): (DateTime<Utc>,) =
                sqlx::query_as("SELECT created_at FROM axom.workers WHERE id = $1").bind(worker_id).fetch_one(pool).await?;
            (created.min(now), None)
        }
        _ => {
            let s = period_start(pool, worker_id).await?;
            (s.min(now), Some(s))
        }
    };
    let m = metrics(pool, worker_id, window_start, now).await?;
    let assessment = performance::assess(&m, &cfg.file.thresholds);
    let id = Uuid::new_v4();
    let spot = trigger != Trigger::Final && performance::spot_check_selected(id, cfg.file.review.spot_check_rate);

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO axom.reviews (id, worker_id, period_start, period_end, trigger, tasks_finished, completed, failed,
            completion_rate, on_time_rate, cost_per_completed_usd, objective_quality, llm_quality, objective_below, reasons, config_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
    )
    .bind(id)
    .bind(worker_id)
    .bind(start)
    .bind(now)
    .bind(trigger.as_str())
    .bind(m.finished() as i32)
    .bind(m.completed as i32)
    .bind(m.failed as i32)
    .bind(m.completion_rate())
    .bind(m.on_time_rate())
    .bind(m.cost_per_completed_usd())
    .bind(m.objective_quality())
    .bind(m.llm_quality)
    .bind(assessment.objective_below)
    .bind(&assessment.reasons)
    .bind(cfg.version())
    .execute(&mut *tx)
    .await?;
    if spot {
        sqlx::query("INSERT INTO axom.spot_checks (id, review_id) VALUES ($1, $2)")
            .bind(Uuid::new_v4())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let ev = AuditEvent::new(&Actor::System, "performance.review", if assessment.objective_below { "below_threshold" } else { "ok" })
        .inputs(json!({ "review_id": id, "worker_id": worker_id, "trigger": trigger.as_str(), "period_start": start,
                        "period_end": now, "metrics": metrics_json(&m), "config_version": cfg.version(), "spot_check": spot }))
        .reasoning(if assessment.reasons.is_empty() { "objective metrics meet thresholds".to_string() } else { assessment.reasons.join("; ") })
        .at(now);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(StoredReview { id, worker_id, period_start: start, period_end: now, trigger, metrics: m, assessment, spot_checked: spot })
}

/// Inputs to the firing rule for a worker, as of `now`.
pub async fn firing_inputs(pool: &PgPool, cfg: &PerformanceConfig, worker_id: Uuid, now: DateTime<Utc>) -> Result<FiringInputs, sqlx::Error> {
    let total = metrics(pool, worker_id, None, now).await?;
    let (first,): (Option<DateTime<Utc>>,) = sqlx::query_as(
        "SELECT min(finished_at) FROM axom.tasks WHERE assignee = $1 AND status IN ('done','failed') AND finished_at <= $2",
    )
    .bind(worker_id)
    .bind(now)
    .fetch_one(pool)
    .await?;
    let data_days = first.map(|f| (now - f).num_seconds() as f64 / 86_400.0).unwrap_or(0.0);
    let recent: Vec<(bool,)> = sqlx::query_as(
        "SELECT objective_below FROM axom.reviews WHERE worker_id = $1 AND trigger <> 'final'
         ORDER BY period_end DESC, created_at DESC LIMIT $2",
    )
    .bind(worker_id)
    .bind(cfg.file.firing.consecutive_below_periods as i64)
    .fetch_all(pool)
    .await?;
    Ok(FiringInputs { completed_total: total.completed, data_days, recent_below: recent.into_iter().map(|r| r.0).collect() })
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewOutcome {
    pub review: StoredReview,
    pub fired: Option<FireReport>,
}

/// Run every due review, move workers on or off probation, and fire workers
/// that meet the firing rule.
pub async fn run_due_reviews(
    pool: &PgPool,
    policy_cfg: &PolicyConfig,
    cfg: &PerformanceConfig,
    notify_cfg: &NotifyConfig,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<ReviewOutcome>> {
    let mut out = Vec::new();
    for w in db::load_workers(pool).await? {
        if !matches!(w.status, WorkerStatus::Active | WorkerStatus::OnProbation) {
            continue;
        }
        let Some(trigger) = review_due(pool, cfg, w.id, now).await? else { continue };
        let review = store_review(pool, cfg, w.id, trigger, now).await?;

        let inputs = firing_inputs(pool, cfg, w.id, now).await?;
        let verdict = performance::firing_verdict(&inputs, &cfg.file.firing);
        if verdict.fire {
            let report = firing::fire_worker(pool, policy_cfg, cfg, notify_cfg, w.id, &verdict.reason, Some(&review), now).await?;
            out.push(ReviewOutcome { review, fired: Some(report) });
            continue;
        }
        // Probation tracks the latest review; it never fires anyone by itself.
        let reason = format!("review {}: {}", review.id, review.assessment.reasons.join("; "));
        if review.assessment.objective_below && w.status == WorkerStatus::Active {
            db::transition(pool, &Actor::System, w.id, WorkerStatus::OnProbation, &reason).await?;
        } else if !review.assessment.objective_below && review.metrics.finished() > 0 && w.status == WorkerStatus::OnProbation {
            db::transition(pool, &Actor::System, w.id, WorkerStatus::Active, "review back above thresholds").await?;
        }
        out.push(ReviewOutcome { review, fired: None });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Owner spot checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpotCheckStats {
    pub reviews: i64,
    pub sampled: i64,
    pub answered: i64,
    pub disagreements: i64,
    /// disagreements / answered, if any were answered.
    pub disagreement_rate: Option<f64>,
}

/// Record the owner's verdict on a sampled review. Admin only.
pub async fn record_spot_check(pool: &PgPool, actor: &Actor, spot_check_id: Uuid, owner_agrees: bool, note: &str) -> anyhow::Result<()> {
    let Actor::Admin { name, mfa_verified: true } = actor else {
        anyhow::bail!("spot checks are answered by an MFA-verified admin in the dashboard");
    };
    let n = sqlx::query(
        "UPDATE axom.spot_checks SET owner_agrees = $2, answered_by = $3, note = $4, answered_at = now()
         WHERE id = $1 AND answered_at IS NULL",
    )
    .bind(spot_check_id)
    .bind(owner_agrees)
    .bind(name)
    .bind(note)
    .execute(pool)
    .await?
    .rows_affected();
    anyhow::ensure!(n == 1, "spot check {spot_check_id} not found or already answered");
    let ev = AuditEvent::new(actor, "performance.spot_check", if owner_agrees { "agree" } else { "disagree" })
        .inputs(json!({ "spot_check_id": spot_check_id, "note": note }));
    audit::append(pool, &ev).await?;
    Ok(())
}

pub async fn spot_check_stats(pool: &PgPool) -> Result<SpotCheckStats, sqlx::Error> {
    let (reviews,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.reviews WHERE trigger <> 'final'").fetch_one(pool).await?;
    let (sampled, answered, disagreements): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), count(owner_agrees), count(*) FILTER (WHERE owner_agrees = false) FROM axom.spot_checks",
    )
    .fetch_one(pool)
    .await?;
    Ok(SpotCheckStats {
        reviews,
        sampled,
        answered,
        disagreements,
        disagreement_rate: (answered > 0).then(|| disagreements as f64 / answered as f64),
    })
}
