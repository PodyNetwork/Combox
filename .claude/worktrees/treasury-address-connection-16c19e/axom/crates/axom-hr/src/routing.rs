//! Task routing: assign each queued task to the active worker whose tools
//! and skills cover it, preferring the lowest current load. If nobody
//! matches, open a hiring request (hiring itself is milestones 7 and 8).

use std::collections::HashMap;

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, transition_in_tx, TransitionDbError};
use axom_core::domain::{Actor, Capabilities, Task, Worker, WorkerKind};
use axom_core::state::TaskStatus;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// Pick the eligible worker with the lowest load. Ties go to the earlier
/// worker in `workers` (callers pass them in creation order).
/// Human workers are never chosen for tool-execution tasks.
pub fn choose_worker<'a>(caps: &Capabilities, workers: &'a [Worker], loads: &HashMap<Uuid, i64>) -> Option<&'a Worker> {
    workers
        .iter()
        .filter(|w| w.status.can_receive_work() && w.covers(caps))
        .min_by_key(|w| loads.get(&w.id).copied().unwrap_or(0))
}

/// Which kind of worker a hiring request should seek. Tool-execution tasks
/// always need an AI agent; otherwise a planned human role that covers the
/// skills means a freelancer, else an AI agent.
pub fn hire_kind(caps: &Capabilities, human_role_skills: &[Vec<String>]) -> WorkerKind {
    if caps.is_tool_execution() {
        return WorkerKind::AiAgent;
    }
    let covered = human_role_skills
        .iter()
        .any(|skills| caps.skills.iter().all(|s| skills.contains(s)));
    if covered {
        WorkerKind::HumanFreelancer
    } else {
        WorkerKind::AiAgent
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RoutingReport {
    pub assigned: Vec<(Uuid, Uuid)>,
    pub hiring_requests: Vec<Uuid>,
    /// Tasks that already had an open hiring request.
    pub awaiting_hire: Vec<Uuid>,
}

/// Route every queued task whose start time has come (scheduled tasks wait
/// until their `start_at`). Serialized across processes with an advisory
/// lock so two routers never double-assign.
pub async fn route_queued(pool: &PgPool, actor: &Actor) -> anyhow::Result<RoutingReport> {
    let mut lock_conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended('axom-routing', 0))").execute(&mut *lock_conn).await?;
    let result = route_queued_locked(pool, actor).await;
    sqlx::query("SELECT pg_advisory_unlock(hashtextextended('axom-routing', 0))").execute(&mut *lock_conn).await?;
    result
}

async fn route_queued_locked(pool: &PgPool, actor: &Actor) -> anyhow::Result<RoutingReport> {
    let workers = db::load_workers(pool).await?;
    let mut loads = db::worker_loads(pool).await?;
    let human_roles: Vec<(Vec<String>,)> =
        sqlx::query_as("SELECT skills FROM axom.roles WHERE kind = 'human_freelancer'").fetch_all(pool).await?;
    let human_role_skills: Vec<Vec<String>> = human_roles.into_iter().map(|r| r.0).collect();

    let mut report = RoutingReport::default();
    for task in db::due_queued_tasks(pool).await? {
        match choose_worker(&task.required_capabilities, &workers, &loads) {
            Some(w) => {
                assign(pool, actor, &task, w).await?;
                *loads.entry(w.id).or_insert(0) += 1;
                report.assigned.push((task.id, w.id));
            }
            None => {
                let kind = hire_kind(&task.required_capabilities, &human_role_skills);
                match open_hiring_request(pool, actor, &task, kind).await? {
                    Some(id) => report.hiring_requests.push(id),
                    None => report.awaiting_hire.push(task.id),
                }
            }
        }
    }
    Ok(report)
}

/// Assign a task to a worker (queued -> assigned), audited.
pub async fn assign(pool: &PgPool, actor: &Actor, task: &Task, worker: &Worker) -> anyhow::Result<()> {
    // Defense in depth: re-check the human/tool rule at the write.
    if worker.kind == WorkerKind::HumanFreelancer && task.required_capabilities.is_tool_execution() {
        anyhow::bail!("refusing to route tool-execution task {} to human worker {}", task.id, worker.id);
    }
    let mut tx = pool.begin().await?;
    match transition_in_tx::<TaskStatus>(&mut tx, task.id, TaskStatus::Assigned).await {
        Ok(_) => {}
        Err(TransitionDbError::Invalid(err)) => {
            tx.rollback().await?;
            db::audit_invalid_transition(pool, actor, "axom.tasks", task.id, &err).await?;
            return Err(err.into());
        }
        Err(e) => return Err(e.into()),
    }
    sqlx::query("UPDATE axom.tasks SET assignee = $2 WHERE id = $1").bind(task.id).bind(worker.id).execute(&mut *tx).await?;
    let ev = AuditEvent::new(actor, "task.route", "assigned")
        .inputs(json!({ "task_id": task.id, "worker_id": worker.id, "worker_kind": worker.kind.as_str(),
                        "required_tools": task.required_capabilities.tools, "required_skills": task.required_capabilities.skills }))
        .reasoning(format!("{} covers the required capabilities with the lowest load", worker.name));
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(())
}

/// Open a hiring request for a task unless one is already open.
/// Returns the new request id, or `None` if one already existed.
pub async fn open_hiring_request(pool: &PgPool, actor: &Actor, task: &Task, kind: WorkerKind) -> anyhow::Result<Option<Uuid>> {
    let tools: Vec<String> = task.required_capabilities.tools.iter().cloned().collect();
    let skills: Vec<String> = task.required_capabilities.skills.iter().cloned().collect();
    insert_request(pool, actor, task.id, kind, &tools, &skills, Extra::default(), "no active worker covers the required capabilities").await
}

/// A freelancer an AI worker asks HR to hire (already checked).
#[derive(Debug, Clone, Default)]
pub struct FreelancerAsk {
    pub title: String,
    pub skills: Vec<String>,
    /// The job post's text.
    pub description: String,
    /// The owner's per-task rate, if they gave one.
    pub rate_usd: Option<Decimal>,
    pub reason: String,
}

/// Open a freelancer hiring request that an AI worker asked for while doing
/// `task_id` (its `request_hire` tool). The usual pipeline takes it from
/// here: a policy-checked job posting, shortlist, interview, and an
/// owner-approved contract. Returns `None` if the task already has an open
/// request.
pub async fn open_freelancer_request(pool: &PgPool, actor: &Actor, task_id: Uuid, ask: &FreelancerAsk) -> anyhow::Result<Option<Uuid>> {
    let extra = Extra { title: Some(&ask.title), description: Some(ask.description.as_str()).filter(|d| !d.is_empty()), rate_usd: ask.rate_usd };
    insert_request(pool, actor, task_id, WorkerKind::HumanFreelancer, &[], &ask.skills, extra, &ask.reason).await
}

/// What a worker's request adds to a routing one.
#[derive(Default)]
struct Extra<'a> {
    title: Option<&'a str>,
    description: Option<&'a str>,
    rate_usd: Option<Decimal>,
}

#[allow(clippy::too_many_arguments)]
async fn insert_request(
    pool: &PgPool,
    actor: &Actor,
    task_id: Uuid,
    kind: WorkerKind,
    tools: &[String],
    skills: &[String],
    extra: Extra<'_>,
    reason: &str,
) -> anyhow::Result<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO axom.hiring_requests (id, task_id, kind, required_tools, required_skills, reason, title, description, rate_usd)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (task_id) WHERE status = 'open' AND task_id IS NOT NULL DO NOTHING",
    )
    .bind(id)
    .bind(task_id)
    .bind(kind.as_str())
    .bind(tools)
    .bind(skills)
    .bind(reason)
    .bind(extra.title)
    .bind(extra.description)
    .bind(extra.rate_usd)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        tx.rollback().await?;
        return Ok(None);
    }
    let ev = AuditEvent::new(actor, "hiring_request.open", "opened")
        .inputs(json!({ "hiring_request_id": id, "task_id": task_id, "kind": kind.as_str(), "required_tools": tools, "required_skills": skills, "title": extra.title, "rate_usd": extra.rate_usd, "description": extra.description }))
        .reasoning(reason);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axom_core::domain::PayUnit;
    use axom_core::state::WorkerStatus;
    use rust_decimal::Decimal;

    fn w(name: &str, kind: WorkerKind, status: WorkerStatus, tools: &[&str], skills: &[&str]) -> Worker {
        Worker {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            role: "r".into(),
            status,
            skills: skills.iter().map(|s| s.to_string()).collect(),
            tool_allowlist: tools.iter().map(|s| s.to_string()).collect(),
            pay_rate_usd: Decimal::ONE,
            pay_unit: PayUnit::Task,
            wallet_address: None,
            kpis: vec![],
        }
    }

    #[test]
    fn picks_lowest_load_matching_worker() {
        let a = w("a", WorkerKind::AiAgent, WorkerStatus::Active, &["code_repo"], &["software_engineering"]);
        let b = w("b", WorkerKind::AiAgent, WorkerStatus::Active, &["code_repo"], &["software_engineering"]);
        let c = w("c", WorkerKind::AiAgent, WorkerStatus::Active, &["postiz"], &["copywriting"]);
        let workers = vec![a.clone(), b.clone(), c];
        let caps = Capabilities::new(["code_repo"], ["software_engineering"]);
        let mut loads = HashMap::from([(a.id, 3), (b.id, 1)]);
        assert_eq!(choose_worker(&caps, &workers, &loads).unwrap().id, b.id);
        loads.insert(b.id, 5);
        assert_eq!(choose_worker(&caps, &workers, &loads).unwrap().id, a.id);
        // Ties go to the earlier worker.
        loads.clear();
        assert_eq!(choose_worker(&caps, &workers, &loads).unwrap().id, a.id);
    }

    #[test]
    fn skips_inactive_and_non_matching() {
        let fired = w("f", WorkerKind::AiAgent, WorkerStatus::Fired, &["code_repo"], &["software_engineering"]);
        let suspended = w("s", WorkerKind::AiAgent, WorkerStatus::Suspended, &["code_repo"], &["software_engineering"]);
        let other = w("o", WorkerKind::AiAgent, WorkerStatus::Active, &["code_repo"], &["testing"]);
        let caps = Capabilities::new(["code_repo"], ["software_engineering"]);
        assert!(choose_worker(&caps, &[fired, suspended, other], &HashMap::new()).is_none());
    }

    #[test]
    fn humans_never_get_tool_tasks() {
        let h = w("h", WorkerKind::HumanFreelancer, WorkerStatus::Active, &[], &["copywriting"]);
        let tool_task = Capabilities::new(["postiz"], ["copywriting"]);
        assert!(choose_worker(&tool_task, std::slice::from_ref(&h), &HashMap::new()).is_none());
        let brief = Capabilities::new(Vec::<String>::new(), ["copywriting"]);
        assert_eq!(choose_worker(&brief, std::slice::from_ref(&h), &HashMap::new()).unwrap().id, h.id);
    }

    #[test]
    fn hire_kind_rules() {
        let human_roles = vec![vec!["visual_design".to_string()]];
        assert_eq!(hire_kind(&Capabilities::new(["postiz"], ["visual_design"]), &human_roles), WorkerKind::AiAgent);
        assert_eq!(hire_kind(&Capabilities::new(Vec::<String>::new(), ["visual_design"]), &human_roles), WorkerKind::HumanFreelancer);
        assert_eq!(hire_kind(&Capabilities::new(Vec::<String>::new(), ["legal"]), &human_roles), WorkerKind::AiAgent);
    }
}
