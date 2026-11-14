//! Firing execution. Autonomous (no human approval), but always:
//! reassign open tasks through routing (hiring requests when nobody can take
//! them), revoke tool access, record reason + metrics + a final review in the
//! audit log, set status fired, and queue the owner notification in the same
//! transaction.

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, transition_in_tx, TransitionDbError};
use axom_core::domain::Actor;
use axom_core::state::{TaskStatus, WorkerStatus};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::notify::{self, NotifyConfig};
use crate::performance::PerformanceConfig;
use crate::policy::{self, Action, Context, PolicyConfig};
use crate::reviews::{self, StoredReview, Trigger};
use crate::routing;

#[derive(Debug, Clone, Serialize)]
pub struct FireReport {
    pub worker_id: Uuid,
    pub audit_id: Uuid,
    pub notification_id: Uuid,
    pub final_review_id: Uuid,
    pub revoked_tools: Vec<String>,
    pub reopened_tasks: Vec<Uuid>,
    /// (task, new worker) for reopened tasks that found a new assignee.
    pub reassigned: Vec<(Uuid, Uuid)>,
    /// Hiring requests opened for reopened tasks nobody could take.
    pub hiring_requests: Vec<Uuid>,
}

#[allow(clippy::too_many_arguments)]
pub async fn fire_worker(
    pool: &PgPool,
    policy_cfg: &PolicyConfig,
    perf_cfg: &PerformanceConfig,
    notify_cfg: &NotifyConfig,
    worker_id: Uuid,
    reason: &str,
    last_review: Option<&StoredReview>,
    now: DateTime<Utc>,
) -> anyhow::Result<FireReport> {
    let actor = Actor::System;
    let worker = db::load_worker(pool, worker_id).await?.ok_or_else(|| anyhow::anyhow!("worker {worker_id} not found"))?;

    let kill: Option<(bool,)> = sqlx::query_as("SELECT kill_switch FROM axom.system_controls WHERE id = 1").fetch_optional(pool).await?;
    let mut ctx = Context::new(actor.clone(), now);
    ctx.kill_switch = kill.map(|k| k.0).unwrap_or(true);
    let decision = policy::evaluate(policy_cfg, &Action::Fire { worker_id, reason: reason.to_string() }, &ctx);
    if !decision.is_allowed() {
        let ev = AuditEvent::new(&actor, "worker.fire", "denied")
            .inputs(json!({ "worker_id": worker_id, "reason": reason }))
            .reasoning(&decision.reason)
            .policy_version(&decision.policy_version)
            .at(now);
        audit::append(pool, &ev).await?;
        anyhow::bail!("firing denied by policy: {}", decision.reason);
    }

    // Final review over the worker's whole history.
    let final_review = reviews::store_review(pool, perf_cfg, worker_id, Trigger::Final, now).await?;
    let metrics = final_review.metrics_json();

    let mut tx = pool.begin().await?;
    match transition_in_tx::<WorkerStatus>(&mut tx, worker_id, WorkerStatus::Fired).await {
        Ok(_) => {}
        Err(TransitionDbError::Invalid(err)) => {
            tx.rollback().await?;
            db::audit_invalid_transition(pool, &actor, "axom.workers", worker_id, &err).await?;
            return Err(err.into());
        }
        Err(e) => return Err(e.into()),
    }
    let revoked_tools: Vec<String> = worker.tool_allowlist.iter().cloned().collect();
    sqlx::query("UPDATE axom.workers SET tool_allowlist = '{}', fired_at = $2 WHERE id = $1")
        .bind(worker_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

    let open: Vec<&str> = TaskStatus::OPEN_FOR_LOAD.iter().map(|s| s.as_str()).collect();
    let open_tasks: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM axom.tasks WHERE assignee = $1 AND status = ANY($2) ORDER BY seq, id FOR UPDATE")
        .bind(worker_id)
        .bind(&open)
        .fetch_all(&mut *tx)
        .await?;
    let reopened: Vec<Uuid> = open_tasks.into_iter().map(|r| r.0).collect();
    for &task_id in &reopened {
        transition_in_tx::<TaskStatus>(&mut tx, task_id, TaskStatus::Queued).await?;
        sqlx::query("UPDATE axom.tasks SET assignee = NULL WHERE id = $1").bind(task_id).execute(&mut *tx).await?;
    }

    let ev = AuditEvent::new(&actor, "worker.fire", "fired")
        .inputs(json!({
            "worker_id": worker_id, "worker_name": worker.name, "kind": worker.kind.as_str(), "role": worker.role,
            "metrics": metrics, "final_review_id": final_review.id,
            "last_review_id": last_review.map(|r| r.id), "last_review_metrics": last_review.map(|r| r.metrics_json()),
            "revoked_tools": revoked_tools, "reopened_tasks": reopened,
            "performance_config_version": perf_cfg.version(),
        }))
        .reasoning(reason)
        .policy_version(&decision.policy_version)
        .at(now);
    let audit_id = audit::append(&mut *tx, &ev).await?;
    let notification_id = notify::enqueue(
        &mut *tx,
        "worker_fired",
        &format!("Worker fired: {} ({})", worker.name, worker.role),
        json!({
            "worker_id": worker_id, "worker_name": worker.name, "kind": worker.kind.as_str(),
            "reason": reason, "metrics": metrics, "audit_link": notify_cfg.audit_link(audit_id),
            "reopened_tasks": reopened.len(), "fired_at": now,
        }),
        Some(audit_id),
    )
    .await?;
    tx.commit().await?;

    // Reassign through routing; tasks nobody can take get hiring requests.
    let routed = routing::route_queued(pool, &actor).await?;
    let reassigned = routed.assigned.iter().copied().filter(|(t, _)| reopened.contains(t)).collect();
    let mut hiring_requests = Vec::new();
    for hr in routed.hiring_requests {
        let (task_id,): (Option<Uuid>,) = sqlx::query_as("SELECT task_id FROM axom.hiring_requests WHERE id = $1").bind(hr).fetch_one(pool).await?;
        if task_id.is_some_and(|t| reopened.contains(&t)) {
            hiring_requests.push(hr);
        }
    }

    Ok(FireReport {
        worker_id,
        audit_id,
        notification_id,
        final_review_id: final_review.id,
        revoked_tools,
        reopened_tasks: reopened,
        reassigned,
        hiring_requests,
    })
}
