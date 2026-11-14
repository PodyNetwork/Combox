//! Runs assigned tasks. Human workers receive a brief through task intake.
//!
//! AI workers: every tool is checked against policy first. Without a Claude
//! key, the task's tools run in order and their outputs are stored as
//! untrusted data, never interpreted. With a key (`thinking`), Claude reads
//! the task, chooses which of the worker's allowed tools to call and with
//! what arguments, and writes up the result. Claude can only call tools that
//! passed the policy check; tool outputs reach it as data, never as
//! instructions; and the kill switch is checked before every call.

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, transition_in_tx, TransitionDbError};
use axom_core::domain::{Actor, Task, WorkerKind};
use axom_core::state::TaskStatus;
use axom_hr::payouts::kill_switch_on;
use axom_hr::policy::{self, Action, Context, PolicyConfig};
use axom_tools::llm::Llm;
use axom_tools::{Authorized, IntakeTask, TaskIntake, ToolExecutor};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunReport {
    pub done: Vec<Uuid>,
    pub sent_to_intake: Vec<Uuid>,
    pub blocked: Vec<Uuid>,
    pub failed: Vec<Uuid>,
}

/// Apply a chain of task transitions plus optional result in one transaction.
pub(crate) async fn move_task(
    pool: &PgPool,
    actor: &Actor,
    task_id: Uuid,
    path: &[TaskStatus],
    result: Option<Value>,
    reason: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for &to in path {
        match transition_in_tx::<TaskStatus>(&mut tx, task_id, to).await {
            Ok(from) => {
                let ev = AuditEvent::new(actor, "task.transition", "applied")
                    .inputs(json!({ "table": "axom.tasks", "id": task_id, "from": from.as_str(), "to": to.as_str() }))
                    .reasoning(reason);
                audit::append(&mut *tx, &ev).await?;
            }
            Err(TransitionDbError::Invalid(err)) => {
                tx.rollback().await?;
                db::audit_invalid_transition(pool, actor, "axom.tasks", task_id, &err).await?;
                return Err(err.into());
            }
            Err(e) => return Err(e.into()),
        }
    }
    let last = path.last().copied();
    let done = last == Some(TaskStatus::Done);
    let finished = done || last == Some(TaskStatus::Failed);
    sqlx::query(
        "UPDATE axom.tasks SET result = COALESCE($2, result),
            completed_at = CASE WHEN $3 THEN now() ELSE completed_at END,
            finished_at = CASE WHEN $4 THEN now() ELSE finished_at END
         WHERE id = $1",
    )
    .bind(task_id)
    .bind(result)
    .bind(done)
    .bind(finished)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// `waiting_for` value of a task that needs an Anthropic key.
pub const ANTHROPIC_KEY: &str = "ANTHROPIC_API_KEY";

pub fn not_connected(tool: &str) -> String {
    format!("{tool} is not connected: add an MCP server or webhook named {tool} on the Tools page")
}

/// Park a task until what it needs is set up (see `resume_waiting`).
pub(crate) async fn wait_for(pool: &PgPool, actor: &Actor, task_id: Uuid, what: &str, why: &str, report: &mut RunReport) -> anyhow::Result<()> {
    move_task(pool, actor, task_id, &[TaskStatus::Blocked], Some(json!({ "waiting_for": what, "error": why })), why).await?;
    report.blocked.push(task_id);
    Ok(())
}

/// Tasks parked for a tool or the Anthropic key go back to their worker
/// once it is there.
async fn resume_waiting(pool: &PgPool, executor: &dyn ToolExecutor, brain: Option<&dyn Llm>) -> anyhow::Result<()> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, result->>'waiting_for' FROM axom.tasks WHERE status = 'blocked' AND assignee IS NOT NULL AND result ? 'waiting_for'",
    )
    .fetch_all(pool)
    .await?;
    for (id, what) in rows {
        // Questions resume when someone replies (crate::questions::reply).
        if what == crate::questions::WAITING_FOR_REPLY {
            continue;
        }
        let ready = if what == ANTHROPIC_KEY { brain.is_some() } else { matches!(executor.resolve(&what).await, Ok(Some(_))) };
        if ready {
            move_task(pool, &Actor::System, id, &[TaskStatus::Assigned], None, &format!("{what} is now set up")).await?;
            sqlx::query("UPDATE axom.tasks SET result = NULL WHERE id = $1").bind(id).execute(pool).await?;
        }
    }
    Ok(())
}

/// Run every assigned task once (AI workers without Claude).
pub async fn run_assigned(
    pool: &PgPool,
    cfg: &PolicyConfig,
    executor: &dyn ToolExecutor,
    intake: &dyn TaskIntake,
) -> anyhow::Result<RunReport> {
    run_assigned_with(pool, cfg, executor, intake, None).await
}

/// Run every assigned task once; AI workers think with `brain` when given.
pub async fn run_assigned_with(
    pool: &PgPool,
    cfg: &PolicyConfig,
    executor: &dyn ToolExecutor,
    intake: &dyn TaskIntake,
    brain: Option<&dyn Llm>,
) -> anyhow::Result<RunReport> {
    let mut report = RunReport::default();
    if let Err(e) = crate::agent_log::prune(pool).await {
        tracing::warn!(error = %e, "agent log cleanup failed");
    }
    // Failed tasks whose automatic retry is due go back to the queue (routed
    // on the next pass).
    match crate::retry::retry_due(pool).await {
        Ok(ids) if !ids.is_empty() => tracing::info!(count = ids.len(), "failed tasks queued again for an automatic retry"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "automatic retry failed"),
    }
    resume_waiting(pool, executor, brain).await?;
    for task in db::tasks_with_status(pool, TaskStatus::Assigned).await? {
        let Some(worker_id) = task.assignee else { continue };
        let Some(worker) = db::load_worker(pool, worker_id).await? else { continue };
        let actor = Actor::agent(&worker.name);
        let mut ctx = Context::new(actor.clone(), Utc::now());
        ctx.kill_switch = kill_switch_on(pool).await?;

        match worker.kind {
            WorkerKind::AiAgent => match brain {
                Some(llm) => crate::thinking::run(pool, cfg, executor, llm, &task, &worker, &actor, &ctx, &mut report).await?,
                // A conversation waiting to carry on needs Claude, not a tools-only run.
                None if crate::questions::has_thread(pool, task.id).await? => {
                    wait_for(pool, &actor, task.id, ANTHROPIC_KEY, "the worker needs an Anthropic key (Integrations, AI models) to carry on with your reply", &mut report).await?
                }
                None => run_ai_task(pool, cfg, executor, &task, &worker.tool_allowlist, &actor, &ctx, &mut report).await?,
            },
            WorkerKind::HumanFreelancer => {
                // Human tasks are briefs, never tool execution (routing enforces this).
                let action = Action::ExecuteTool { tool: "task_intake".into(), worker_allowlist: vec!["task_intake".into()] };
                let decision = policy::evaluate(cfg, &action, &Context { actor: Actor::System, ..ctx.clone() });
                let Some(auth) = Authorized::from_decision("task_intake.submit", &decision) else {
                    move_task(pool, &Actor::System, task.id, &[TaskStatus::Blocked], None, &decision.reason).await?;
                    report.blocked.push(task.id);
                    continue;
                };
                let brief = IntakeTask {
                    task_id: task.id,
                    brief: task.objective.clone(),
                    required_skills: task.required_capabilities.skills.iter().cloned().collect(),
                    assignee: Some(worker.id),
                };
                let position = intake.submit(&auth, &brief).await?;
                move_task(pool, &Actor::System, task.id, &[TaskStatus::InProgress], Some(json!({ "intake_position": position })), "brief sent through task intake").await?;
                report.sent_to_intake.push(task.id);
            }
        }
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn run_ai_task(
    pool: &PgPool,
    cfg: &PolicyConfig,
    executor: &dyn ToolExecutor,
    task: &Task,
    allowlist: &std::collections::BTreeSet<String>,
    actor: &Actor,
    ctx: &Context,
    report: &mut RunReport,
) -> anyhow::Result<()> {
    let allow: Vec<String> = allowlist.iter().cloned().collect();
    // Check every tool before starting, so a denied tool blocks the task cleanly.
    let mut auths = Vec::new();
    for tool in &task.required_capabilities.tools {
        let decision = policy::evaluate(cfg, &Action::ExecuteTool { tool: tool.clone(), worker_allowlist: allow.clone() }, ctx);
        match Authorized::from_decision("execute_tool", &decision) {
            Some(a) => auths.push((tool.clone(), a)),
            None => {
                let ev = AuditEvent::new(actor, "tool.execute", "denied")
                    .inputs(json!({ "task_id": task.id, "tool": tool }))
                    .reasoning(&decision.reason)
                    .policy_version(&decision.policy_version);
                audit::append(pool, &ev).await?;
                move_task(pool, actor, task.id, &[TaskStatus::Blocked], None, &decision.reason).await?;
                report.blocked.push(task.id);
                return Ok(());
            }
        }
    }

    // Each needed tool must be connected; an MCP server with several tools
    // needs Claude to choose among them.
    let mut calls = Vec::new();
    for (tool, auth) in auths {
        match executor.resolve(&tool).await {
            Ok(Some(specs)) if specs.len() == 1 => calls.push((specs[0].id.clone(), tool, auth)),
            Ok(Some(_)) => return wait_for(pool, actor, task.id, ANTHROPIC_KEY, &format!("{tool} has several tools: set an Anthropic key (Integrations, AI models) so the worker can choose"), report).await,
            Ok(None) => return wait_for(pool, actor, task.id, &tool, &not_connected(&tool), report).await,
            Err(e) => {
                move_task(pool, actor, task.id, &[TaskStatus::Failed], Some(json!({ "error": e.to_string(), "retryable": true })), "tool lookup failed").await?;
                report.failed.push(task.id);
                return Ok(());
            }
        }
    }
    move_task(pool, actor, task.id, &[TaskStatus::InProgress], None, "worker started task").await?;
    let mut outputs = Vec::new();
    for (id, tool, auth) in &calls {
        match executor.execute(auth, id, json!({ "task_id": task.id, "objective": task.objective })).await {
            Ok(out) => {
                let ev = AuditEvent::new(actor, "tool.execute", "ok")
                    .inputs(json!({ "task_id": task.id, "tool": tool }))
                    .policy_version(auth.policy_version());
                audit::append(pool, &ev).await?;
                // Stored as data only. Nothing here parses or acts on it.
                outputs.push(json!({ "source": out.source, "untrusted_content": out.raw() }));
            }
            Err(e) => {
                let ev = AuditEvent::new(actor, "tool.execute", "error")
                    .inputs(json!({ "task_id": task.id, "tool": tool }))
                    .reasoning(e.to_string())
                    .policy_version(auth.policy_version());
                audit::append(pool, &ev).await?;
                move_task(pool, actor, task.id, &[TaskStatus::Failed], Some(json!({ "error": e.to_string(), "retryable": true })), "tool error").await?;
                report.failed.push(task.id);
                return Ok(());
            }
        }
    }
    move_task(pool, actor, task.id, &[TaskStatus::Done], Some(json!({ "tool_outputs": outputs })), "all tools completed").await?;
    report.done.push(task.id);
    Ok(())
}
