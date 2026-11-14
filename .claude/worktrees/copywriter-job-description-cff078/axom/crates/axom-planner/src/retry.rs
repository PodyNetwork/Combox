//! Retrying failed tasks: by hand from the dashboard, or automatically.
//!
//! A retry sends the task back to the queue (an audited `failed -> queued`
//! transition), clears its assignee and result, and lets routing give it to
//! a worker again.
//!
//! Automatic retries apply only to failures marked `retryable` when they
//! happened (Claude API and tool errors, which may pass on another try), at
//! most [`MAX_AUTO_RETRIES`] times, waiting longer each time. A task Claude
//! declined, or that ran out of steps, is left for a person to look at. An
//! admin's retry starts the count again. Nothing is retried while the kill
//! switch is on.

use std::collections::HashMap;

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, transition_in_tx, TransitionDbError};
use axom_core::domain::Actor;
use axom_core::state::TaskStatus;
use axom_hr::payouts::kill_switch_on;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// Automatic retries per task before it waits for a person.
pub const MAX_AUTO_RETRIES: i32 = 3;

/// How long after a failure each automatic retry waits, by retries so far:
/// 5 minutes, 30 minutes, then 2 hours.
const BACKOFF_SQL: &str = "make_interval(secs => CASE retries WHEN 0 THEN 300 WHEN 1 THEN 1800 ELSE 7200 END)";

#[derive(Debug, thiserror::Error)]
pub enum RetryError {
    #[error("task not found")]
    NotFound,
    #[error("only a failed task can be retried; this one is {0}")]
    NotFailed(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The next automatic retry planned for a failed task.
#[derive(Debug, Clone, Serialize)]
pub struct Planned {
    /// Which retry this will be (1-based), out of `max`.
    pub attempt: i32,
    pub max: i32,
    pub at: DateTime<Utc>,
}

/// Send a failed task back to the queue. `automatic` retries count toward
/// [`MAX_AUTO_RETRIES`]; a person's retry resets the count.
pub async fn requeue(pool: &PgPool, actor: &Actor, task_id: Uuid, reason: &str, automatic: bool) -> Result<(), RetryError> {
    let mut tx = pool.begin().await?;
    match transition_in_tx::<TaskStatus>(&mut tx, task_id, TaskStatus::Queued).await {
        Ok(from) => {
            let ev = AuditEvent::new(actor, "task.transition", "applied")
                .inputs(json!({ "table": "axom.tasks", "id": task_id, "from": from.as_str(), "to": "queued" }))
                .reasoning(reason);
            audit::append(&mut *tx, &ev).await?;
        }
        Err(TransitionDbError::Invalid(err)) => {
            tx.rollback().await?;
            db::audit_invalid_transition(pool, actor, "axom.tasks", task_id, &err).await?;
            return Err(RetryError::NotFailed(err.from));
        }
        Err(TransitionDbError::NotFound { .. }) => return Err(RetryError::NotFound),
        Err(TransitionDbError::Db(e)) => return Err(e.into()),
    }
    let (retries,): (i32,) = sqlx::query_as(
        "UPDATE axom.tasks SET assignee = NULL, result = NULL, completed_at = NULL, finished_at = NULL,
            retries = CASE WHEN $2 THEN retries + 1 ELSE 0 END
         WHERE id = $1 RETURNING retries",
    )
    .bind(task_id)
    .bind(automatic)
    .fetch_one(&mut *tx)
    .await?;
    let ev = AuditEvent::new(actor, "task.retry", "queued").inputs(json!({ "task_id": task_id, "automatic": automatic, "auto_retries": retries })).reasoning(reason);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    // Starting over: an old open question no longer applies.
    crate::questions::discard(pool, task_id).await?;
    Ok(())
}

/// Requeue every failed task whose automatic retry is due. Returns their ids.
pub async fn retry_due(pool: &PgPool) -> anyhow::Result<Vec<Uuid>> {
    if kill_switch_on(pool).await? {
        return Ok(Vec::new());
    }
    let due: Vec<(Uuid, i32)> = sqlx::query_as(&format!(
        "SELECT id, retries FROM axom.tasks
         WHERE status = 'failed' AND result->>'retryable' = 'true' AND retries < $1
           AND finished_at + {BACKOFF_SQL} <= now()
         ORDER BY finished_at"
    ))
    .bind(MAX_AUTO_RETRIES)
    .fetch_all(pool)
    .await?;
    let mut done = Vec::new();
    for (id, retries) in due {
        let reason = format!("automatic retry {} of {MAX_AUTO_RETRIES}", retries + 1);
        match requeue(pool, &Actor::System, id, &reason, true).await {
            Ok(()) => done.push(id),
            // Someone retried or changed it in the meantime.
            Err(RetryError::NotFailed(_) | RetryError::NotFound) => {}
            Err(RetryError::Db(e)) => return Err(e.into()),
        }
    }
    Ok(done)
}

/// Planned automatic retries for failed tasks, by task id.
pub async fn planned(pool: &PgPool) -> sqlx::Result<HashMap<Uuid, Planned>> {
    let rows: Vec<(Uuid, i32, DateTime<Utc>)> = sqlx::query_as(&format!(
        "SELECT id, retries, finished_at + {BACKOFF_SQL} FROM axom.tasks
         WHERE status = 'failed' AND result->>'retryable' = 'true' AND retries < $1 AND finished_at IS NOT NULL"
    ))
    .bind(MAX_AUTO_RETRIES)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id, retries, at)| (id, Planned { attempt: retries + 1, max: MAX_AUTO_RETRIES, at })).collect())
}
