//! Database helpers for workers, tasks, and audited status transitions.

use std::collections::HashMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::postgres::PgRow;
use sqlx::{PgExecutor, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::audit::{self, AuditEvent};
use crate::domain::{parse_db, Actor, Capabilities, Kpi, ParseEnumError, Task, Worker};
use crate::state::{PayoutStatus, StateMachine, TaskStatus, TransitionError, WorkerStatus};

#[derive(Debug, thiserror::Error)]
pub enum TransitionDbError {
    #[error("{table} {id} not found")]
    NotFound { table: &'static str, id: Uuid },
    #[error(transparent)]
    Invalid(#[from] TransitionError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Tables with a `status` column driven by a state machine.
pub trait StatusTable: StateMachine + FromStr<Err = ParseEnumError> {
    const TABLE: &'static str;
    fn db_str(&self) -> &'static str;
}

impl StatusTable for TaskStatus {
    const TABLE: &'static str = "axom.tasks";
    fn db_str(&self) -> &'static str {
        self.as_str()
    }
}
impl StatusTable for WorkerStatus {
    const TABLE: &'static str = "axom.workers";
    fn db_str(&self) -> &'static str {
        self.as_str()
    }
}
impl StatusTable for PayoutStatus {
    const TABLE: &'static str = "axom.payouts";
    fn db_str(&self) -> &'static str {
        self.as_str()
    }
}

/// Lock the row, validate the transition, and update the status inside `tx`.
/// Does not audit; callers that roll back on error should use
/// [`audit_invalid_transition`] afterwards so the attempt is still recorded.
pub async fn transition_in_tx<S: StatusTable>(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    to: S,
) -> Result<S, TransitionDbError> {
    let sql = format!("SELECT status FROM {} WHERE id = $1 FOR UPDATE", S::TABLE);
    let row: Option<(String,)> = sqlx::query_as(&sql).bind(id).fetch_optional(&mut **tx).await?;
    let (current,) = row.ok_or(TransitionDbError::NotFound { table: S::TABLE, id })?;
    let from: S = parse_db(&current)?;
    from.transition(to)?;
    let sql = format!("UPDATE {} SET status = $2, updated_at = now() WHERE id = $1", S::TABLE);
    sqlx::query(&sql).bind(id).bind(to.db_str()).execute(&mut **tx).await?;
    Ok(from)
}

/// Record a rejected transition in the audit log.
pub async fn audit_invalid_transition<'e, E: PgExecutor<'e>>(
    ex: E,
    actor: &Actor,
    table: &str,
    id: Uuid,
    err: &TransitionError,
) -> Result<(), sqlx::Error> {
    let ev = AuditEvent::new(actor, format!("{}.transition", err.machine), "rejected")
        .inputs(json!({ "table": table, "id": id, "from": err.from, "to": err.to }))
        .reasoning(err.to_string());
    audit::append(ex, &ev).await.map(|_| ())
}

/// Transition a status in its own transaction. Valid transitions are audited
/// as `applied`; invalid ones return an error and are audited as `rejected`.
pub async fn transition<S: StatusTable>(
    pool: &PgPool,
    actor: &Actor,
    id: Uuid,
    to: S,
    reason: &str,
) -> Result<S, TransitionDbError> {
    let mut tx = pool.begin().await?;
    match transition_in_tx::<S>(&mut tx, id, to).await {
        Ok(from) => {
            let ev = AuditEvent::new(actor, format!("{}.transition", S::NAME), "applied")
                .inputs(json!({ "table": S::TABLE, "id": id, "from": from.to_string(), "to": to.to_string() }))
                .reasoning(reason);
            audit::append(&mut *tx, &ev).await?;
            tx.commit().await?;
            Ok(from)
        }
        Err(TransitionDbError::Invalid(err)) => {
            tx.rollback().await?;
            audit_invalid_transition(pool, actor, S::TABLE, id, &err).await?;
            Err(TransitionDbError::Invalid(err))
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

fn vec_of(set: &std::collections::BTreeSet<String>) -> Vec<String> {
    set.iter().cloned().collect()
}

pub async fn insert_worker<'e, E: PgExecutor<'e>>(
    ex: E,
    team_id: Option<Uuid>,
    role_id: Option<Uuid>,
    w: &Worker,
) -> Result<(), sqlx::Error> {
    WorkerStatus::start(w.status).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    sqlx::query(
        "INSERT INTO axom.workers
           (id, team_id, role_id, name, kind, role, status, skills, tool_allowlist,
            pay_rate_usd, pay_unit, wallet_address, kpis)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(w.id)
    .bind(team_id)
    .bind(role_id)
    .bind(&w.name)
    .bind(w.kind.as_str())
    .bind(&w.role)
    .bind(w.status.as_str())
    .bind(vec_of(&w.skills))
    .bind(vec_of(&w.tool_allowlist))
    .bind(w.pay_rate_usd)
    .bind(w.pay_unit.as_str())
    .bind(&w.wallet_address)
    .bind(serde_json::to_value(&w.kpis).unwrap_or_default())
    .execute(ex)
    .await?;
    Ok(())
}

const WORKER_COLS: &str = "id, name, kind, role, status, skills, tool_allowlist, pay_rate_usd, pay_unit, wallet_address, kpis";

fn worker_from_row(r: &PgRow) -> Result<Worker, sqlx::Error> {
    let skills: Vec<String> = r.try_get("skills")?;
    let tools: Vec<String> = r.try_get("tool_allowlist")?;
    let kpis: serde_json::Value = r.try_get("kpis")?;
    Ok(Worker {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        kind: parse_db(r.try_get::<&str, _>("kind")?)?,
        role: r.try_get("role")?,
        status: parse_db(r.try_get::<&str, _>("status")?)?,
        skills: skills.into_iter().collect(),
        tool_allowlist: tools.into_iter().collect(),
        pay_rate_usd: r.try_get::<Decimal, _>("pay_rate_usd")?,
        pay_unit: parse_db(r.try_get::<&str, _>("pay_unit")?)?,
        wallet_address: r.try_get("wallet_address")?,
        kpis: serde_json::from_value::<Vec<Kpi>>(kpis).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
    })
}

pub async fn load_workers<'e, E: PgExecutor<'e>>(ex: E) -> Result<Vec<Worker>, sqlx::Error> {
    let sql = format!("SELECT {WORKER_COLS} FROM axom.workers ORDER BY created_at, id");
    sqlx::query(&sql).fetch_all(ex).await?.iter().map(worker_from_row).collect()
}

pub async fn load_worker<'e, E: PgExecutor<'e>>(ex: E, id: Uuid) -> Result<Option<Worker>, sqlx::Error> {
    let sql = format!("SELECT {WORKER_COLS} FROM axom.workers WHERE id = $1");
    sqlx::query(&sql).bind(id).fetch_optional(ex).await?.as_ref().map(worker_from_row).transpose()
}

/// Current load (open assigned tasks) per worker.
pub async fn worker_loads<'e, E: PgExecutor<'e>>(ex: E) -> Result<HashMap<Uuid, i64>, sqlx::Error> {
    let open: Vec<&str> = TaskStatus::OPEN_FOR_LOAD.iter().map(|s| s.as_str()).collect();
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT assignee, count(*) FROM axom.tasks
         WHERE assignee IS NOT NULL AND status = ANY($1) GROUP BY assignee",
    )
    .bind(&open)
    .fetch_all(ex)
    .await?;
    Ok(rows.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

/// Extra placement info for a task created from a plan.
#[derive(Debug, Clone, Default)]
pub struct TaskPlacement {
    pub plan_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub seq: i32,
}

pub async fn insert_task<'e, E: PgExecutor<'e>>(
    ex: E,
    t: &Task,
    placement: &TaskPlacement,
) -> Result<(), sqlx::Error> {
    TaskStatus::start(t.status).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    sqlx::query(
        "INSERT INTO axom.tasks
           (id, plan_id, department_id, seq, objective, required_tools, required_skills,
            priority, deadline, assignee, status, result, cost_usd, start_at, worker_kind)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13, COALESCE($14, now()), $15)",
    )
    .bind(t.id)
    .bind(placement.plan_id)
    .bind(placement.department_id)
    .bind(placement.seq)
    .bind(&t.objective)
    .bind(vec_of(&t.required_capabilities.tools))
    .bind(vec_of(&t.required_capabilities.skills))
    .bind(t.priority)
    .bind(t.deadline)
    .bind(t.assignee)
    .bind(t.status.as_str())
    .bind(&t.result)
    .bind(t.cost_usd)
    .bind(t.start_at)
    .bind(t.worker_kind.map(|k| k.as_str()))
    .execute(ex)
    .await?;
    Ok(())
}

const TASK_COLS: &str =
    "id, objective, required_tools, required_skills, priority, deadline, assignee, status, result, cost_usd, start_at, worker_kind";

fn task_from_row(r: &PgRow) -> Result<Task, sqlx::Error> {
    let tools: Vec<String> = r.try_get("required_tools")?;
    let skills: Vec<String> = r.try_get("required_skills")?;
    Ok(Task {
        id: r.try_get("id")?,
        objective: r.try_get("objective")?,
        required_capabilities: Capabilities::new(tools, skills),
        priority: r.try_get("priority")?,
        deadline: r.try_get("deadline")?,
        assignee: r.try_get("assignee")?,
        status: parse_db(r.try_get::<&str, _>("status")?)?,
        result: r.try_get("result")?,
        cost_usd: r.try_get("cost_usd")?,
        start_at: Some(r.try_get("start_at")?),
        worker_kind: r.try_get::<Option<&str>, _>("worker_kind")?.map(parse_db).transpose()?,
    })
}

pub async fn load_task<'e, E: PgExecutor<'e>>(ex: E, id: Uuid) -> Result<Option<Task>, sqlx::Error> {
    let sql = format!("SELECT {TASK_COLS} FROM axom.tasks WHERE id = $1");
    sqlx::query(&sql).bind(id).fetch_optional(ex).await?.as_ref().map(task_from_row).transpose()
}

/// Tasks with a given status, highest priority first, then by plan order.
pub async fn tasks_with_status<'e, E: PgExecutor<'e>>(ex: E, status: TaskStatus) -> Result<Vec<Task>, sqlx::Error> {
    let sql = format!(
        "SELECT {TASK_COLS} FROM axom.tasks WHERE status = $1 ORDER BY priority, seq, created_at, id"
    );
    sqlx::query(&sql).bind(status.as_str()).fetch_all(ex).await?.iter().map(task_from_row).collect()
}

/// Queued tasks whose start time has come, in routing order.
pub async fn due_queued_tasks<'e, E: PgExecutor<'e>>(ex: E) -> Result<Vec<Task>, sqlx::Error> {
    let sql = format!(
        "SELECT {TASK_COLS} FROM axom.tasks WHERE status = 'queued' AND start_at <= now() ORDER BY priority, seq, created_at, id"
    );
    sqlx::query(&sql).fetch_all(ex).await?.iter().map(task_from_row).collect()
}

/// When the next scheduled (queued, not yet due) task starts, if any.
pub async fn next_task_start<'e, E: PgExecutor<'e>>(ex: E) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    sqlx::query_scalar("SELECT min(start_at) FROM axom.tasks WHERE status = 'queued' AND start_at > now()").fetch_one(ex).await
}

pub async fn tasks_for_plan<'e, E: PgExecutor<'e>>(ex: E, plan_id: Uuid) -> Result<Vec<Task>, sqlx::Error> {
    let sql = format!("SELECT {TASK_COLS} FROM axom.tasks WHERE plan_id = $1 ORDER BY seq");
    sqlx::query(&sql).bind(plan_id).fetch_all(ex).await?.iter().map(task_from_row).collect()
}
