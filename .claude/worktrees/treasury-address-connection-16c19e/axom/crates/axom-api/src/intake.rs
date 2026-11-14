//! Task intake: an internal queue inside axom-api.
//!
//! - Needs-based submission: `POST /intake/tasks` accepts a task with its
//!   requirements and an optional `start_at` (default: now), and returns its
//!   queue position (internal token or admin). Work starts as soon as the
//!   task is due: the company's loop is woken when it is queued.
//! - Freelancer briefs: human workers see their briefs at `GET /intake/briefs`
//!   and submit deliverables at `POST /intake/briefs/{task_id}/deliverables`
//!   (per-worker bearer token). Deliverables are untrusted data.
//! - Review: an MFA-verified admin accepts or returns a deliverable at
//!   `POST /intake/briefs/{task_id}/review`. Accepting completes the task
//!   (which feeds payroll) and records the acceptance-criteria check (which
//!   feeds performance reviews).

use async_trait::async_trait;
use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, TaskPlacement};
use axom_core::domain::{Actor, Capabilities, Task};
use axom_core::state::TaskStatus;
use axom_tools::{Authorized, IntakeTask, TaskIntake, ToolError, ToolResult};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::admin::mfa_admin;
use crate::{reply, AppState};

/// The `TaskIntake` adapter backed by the intake table.
pub struct DbTaskIntake {
    pub pool: PgPool,
}

const INTAKE_ACTIONS: &[&str] = &["task_intake.submit", "execute_tool"];

#[async_trait]
impl TaskIntake for DbTaskIntake {
    async fn submit(&self, auth: &Authorized, task: &IntakeTask) -> ToolResult<u64> {
        if !INTAKE_ACTIONS.contains(&auth.action()) {
            return Err(ToolError::NotAuthorized(format!("task intake does not accept a decision for {}", auth.action())));
        }
        let db = |e: sqlx::Error| ToolError::Retryable(format!("intake db: {e}"));
        sqlx::query(
            "INSERT INTO axom.intake_items (id, task_id, brief, required_skills, assignee) VALUES ($1,$2,$3,$4,$5)
             ON CONFLICT (task_id) DO UPDATE SET brief = EXCLUDED.brief, assignee = EXCLUDED.assignee",
        )
        .bind(Uuid::new_v4())
        .bind(task.task_id)
        .bind(&task.brief)
        .bind(&task.required_skills)
        .bind(task.assignee)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        let (pos,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM axom.intake_items i
             WHERE i.status IN ('open','returned') AND i.assignee IS NOT DISTINCT FROM $1
               AND i.created_at <= (SELECT created_at FROM axom.intake_items WHERE task_id = $2)",
        )
        .bind(task.assignee)
        .bind(task.task_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(pos as u64)
    }
}

// ---------------------------------------------------------------------------
// Worker tokens
// ---------------------------------------------------------------------------

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Issue a new intake token for a worker. Only the hash is stored; the token
/// is returned once.
pub async fn issue_worker_token(pool: &PgPool, worker_id: Uuid) -> Result<String, sqlx::Error> {
    axom_hr::hiring::issue_intake_token(pool, worker_id).await
}

pub(crate) async fn worker_from_headers(pool: &PgPool, headers: &HeaderMap) -> Result<Uuid, Box<Response>> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| t.starts_with("wk_"));
    let Some(token) = token else {
        return Err(Box::new(reply(StatusCode::UNAUTHORIZED, json!({ "error": "worker token required" }))));
    };
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT t.worker_id, w.status FROM axom.worker_tokens t JOIN axom.workers w ON w.id = t.worker_id
         WHERE t.token_sha256 = $1 AND t.revoked_at IS NULL",
    )
    .bind(hash(token))
    .fetch_optional(pool)
    .await
    .map_err(|_| reply(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "internal error" })))?;
    match row {
        Some((id, status)) if status == "active" || status == "on_probation" => Ok(id),
        Some(_) => Err(Box::new(reply(StatusCode::FORBIDDEN, json!({ "error": "worker is not active" })))),
        None => Err(Box::new(reply(StatusCode::UNAUTHORIZED, json!({ "error": "invalid worker token" })))),
    }
}

// ---------------------------------------------------------------------------
// Needs-based submission
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewTask {
    pub objective: String,
    #[serde(default)]
    pub required_skills: Vec<String>,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default)]
    pub deadline: Option<NaiveDate>,
    /// When work may start (RFC 3339). Missing or past: now.
    #[serde(default)]
    pub start_at: Option<DateTime<Utc>>,
}

/// How far ahead a task can be scheduled.
const MAX_SCHEDULE_DAYS: i64 = 365;

fn default_priority() -> i32 {
    3
}

fn internal_ok(s: &AppState, headers: &HeaderMap) -> bool {
    let given = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    match (&s.internal_token, given) {
        // Constant-time enough for a long random token compared once per request.
        (Some(expected), Some(g)) => hash(expected) == hash(g),
        _ => false,
    }
}

pub async fn submit_task(State(s): State<AppState>, headers: HeaderMap, Json(body): Json<NewTask>) -> Response {
    let actor = if internal_ok(&s, &headers) {
        Actor::System
    } else {
        // Queued work runs with the company's tools and budget, so only an
        // MFA-verified admin of this company may add it.
        match mfa_admin(&s, &headers).await {
            Ok((_, actor)) => actor,
            Err(r) => return *r,
        }
    };
    let objective = body.objective.trim();
    let tag_ok = |t: &String| !t.trim().is_empty() && t.len() <= 100;
    if objective.is_empty()
        || objective.len() > 2000
        || !(1..=5).contains(&body.priority)
        || body.required_skills.len() > 20
        || body.required_tools.len() > 20
        || !body.required_skills.iter().chain(&body.required_tools).all(tag_ok)
    {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "objective 1-2000 chars, priority 1-5, at most 20 skills and 20 tools (each 1-100 chars)" }));
    }
    let now = Utc::now();
    // A start time in the past means now.
    let start_at = body.start_at.filter(|t| *t > now);
    if start_at.is_some_and(|t| t > now + chrono::Duration::days(MAX_SCHEDULE_DAYS)) {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": format!("start_at can be at most {MAX_SCHEDULE_DAYS} days ahead") }));
    }
    // A task that needs tools can lead to an AI worker being hired with them
    // (when no planned role covers it), so only tools that exist and are
    // switched on may be asked for.
    match axom_hr::custom_tools::available_tools(&s.pool).await {
        Ok(available) => {
            if let Some(unknown) = body.required_tools.iter().find(|t| !available.contains(t)) {
                return reply(StatusCode::BAD_REQUEST, json!({ "error": format!("unknown or disabled tool: {unknown}") }));
            }
        }
        Err(e) => return crate::ApiError(e.to_string()).into_response(),
    }
    let task = Task {
        id: Uuid::new_v4(),
        objective: objective.to_string(),
        required_capabilities: Capabilities::new(body.required_tools.clone(), body.required_skills.clone()),
        priority: body.priority,
        deadline: body.deadline,
        assignee: None,
        status: TaskStatus::Queued,
        result: None,
        cost_usd: Decimal::ZERO,
        start_at,
    };
    let result = async {
        db::insert_task(&s.pool, &task, &TaskPlacement::default()).await?;
        let ev = AuditEvent::new(&actor, "intake.submit", "queued")
            .inputs(json!({ "task_id": task.id, "objective": task.objective, "required_skills": body.required_skills, "required_tools": body.required_tools, "priority": task.priority, "start_at": start_at }));
        audit::append(&s.pool, &ev).await?;
        let (pos,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM axom.tasks t, axom.tasks me WHERE me.id = $1 AND t.status = 'queued'
               AND (t.priority < me.priority OR (t.priority = me.priority AND (t.created_at, t.id) <= (me.created_at, me.id)))",
        )
        .bind(task.id)
        .fetch_one(&s.pool)
        .await?;
        Ok::<i64, sqlx::Error>(pos)
    }
    .await;
    match result {
        Ok(pos) => reply(StatusCode::CREATED, json!({ "task_id": task.id, "queue_position": pos, "start_at": start_at.unwrap_or(now), "scheduled": start_at.is_some() })),
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Freelancer briefs and deliverables
// ---------------------------------------------------------------------------

type BriefRow = (Uuid, String, Vec<String>, String, Option<serde_json::Value>, Option<String>, chrono::DateTime<Utc>);

pub async fn my_briefs(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    let rows: Result<Vec<BriefRow>, _> = sqlx::query_as(
        "SELECT task_id, brief, required_skills, status, deliverable, review_note, created_at
         FROM axom.intake_items WHERE assignee = $1 ORDER BY created_at",
    )
    .bind(worker)
    .fetch_all(&s.pool)
    .await;
    match rows {
        Ok(rows) => {
            let briefs: Vec<_> = rows
                .into_iter()
                .map(|r| json!({ "task_id": r.0, "brief": r.1, "required_skills": r.2, "status": r.3, "deliverable": r.4, "review_note": r.5, "created_at": r.6 }))
                .collect();
            reply(StatusCode::OK, json!({ "briefs": briefs }))
        }
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct Deliverable {
    pub url: String,
    #[serde(default)]
    pub notes: String,
}

pub async fn submit_deliverable(State(s): State<AppState>, headers: HeaderMap, Path(task_id): Path<Uuid>, Json(d): Json<Deliverable>) -> Response {
    let worker = match worker_from_headers(&s.pool, &headers).await {
        Ok(w) => w,
        Err(r) => return *r,
    };
    let url_ok = (d.url.starts_with("https://") || d.url.starts_with("http://")) && d.url.len() <= 2000 && !d.url.contains(char::is_whitespace);
    if !url_ok || d.notes.len() > 5000 {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "url must be http(s), at most 2000 chars; notes at most 5000 chars" }));
    }
    let deliverable = json!({ "untrusted": true, "source": "freelancer_deliverable", "url": d.url, "notes": d.notes });
    let n = sqlx::query(
        "UPDATE axom.intake_items SET deliverable = $3, status = 'submitted', submitted_at = now()
         WHERE task_id = $1 AND assignee = $2 AND status IN ('open','returned')",
    )
    .bind(task_id)
    .bind(worker)
    .bind(&deliverable)
    .execute(&s.pool)
    .await;
    match n {
        Ok(r) if r.rows_affected() == 1 => {
            let ev = AuditEvent::new(&Actor::agent(format!("worker:{worker}")), "intake.deliverable", "submitted")
                .inputs(json!({ "task_id": task_id, "worker_id": worker, "deliverable": deliverable }));
            let _ = audit::append(&s.pool, &ev).await;
            reply(StatusCode::OK, json!({ "task_id": task_id, "status": "submitted" }))
        }
        Ok(_) => reply(StatusCode::NOT_FOUND, json!({ "error": "no open brief for this task" })),
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct Review {
    pub accept: bool,
    /// Whether the deliverable met the task's acceptance criteria.
    #[serde(default)]
    pub criteria_met: bool,
    #[serde(default)]
    pub note: String,
}

pub async fn review_deliverable(State(s): State<AppState>, headers: HeaderMap, Path(task_id): Path<Uuid>, Json(r): Json<Review>) -> Response {
    // Accepting a deliverable completes the task, which feeds payroll.
    let (session, actor) = match mfa_admin(&s, &headers).await {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let res: anyhow::Result<Option<&str>> = async {
        let mut tx = s.pool.begin().await?;
        let row: Option<(String,)> = sqlx::query_as("SELECT status FROM axom.intake_items WHERE task_id = $1 FOR UPDATE").bind(task_id).fetch_optional(&mut *tx).await?;
        match row {
            Some((st,)) if st == "submitted" => {}
            _ => return Ok(None),
        }
        let new_status = if r.accept { "accepted" } else { "returned" };
        sqlx::query("UPDATE axom.intake_items SET status = $2, review_note = $3, reviewed_at = now(), reviewed_by = $4 WHERE task_id = $1")
            .bind(task_id)
            .bind(new_status)
            .bind(&r.note)
            .bind(&session.email)
            .execute(&mut *tx)
            .await?;
        if r.accept {
            db::transition_in_tx::<TaskStatus>(&mut tx, task_id, TaskStatus::Done).await?;
            sqlx::query("UPDATE axom.tasks SET completed_at = now(), finished_at = now(), objective_check_passed = $2 WHERE id = $1")
                .bind(task_id)
                .bind(r.criteria_met)
                .execute(&mut *tx)
                .await?;
        }
        let ev = AuditEvent::new(&actor, "intake.review", new_status).inputs(json!({ "task_id": task_id, "criteria_met": r.criteria_met, "note": r.note }));
        audit::append(&mut *tx, &ev).await?;
        tx.commit().await?;
        Ok(Some(new_status))
    }
    .await;
    match res {
        Ok(Some(st)) => reply(StatusCode::OK, json!({ "task_id": task_id, "status": st })),
        Ok(None) => reply(StatusCode::CONFLICT, json!({ "error": "no submitted deliverable to review" })),
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

pub async fn new_worker_token(State(s): State<AppState>, headers: HeaderMap, Path(worker_id): Path<Uuid>) -> Response {
    let actor = match mfa_admin(&s, &headers).await {
        Ok((_, actor)) => actor,
        Err(resp) => return *resp,
    };
    match db::load_worker(&s.pool, worker_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return reply(StatusCode::NOT_FOUND, json!({ "error": "worker not found" })),
        Err(e) => return crate::ApiError(e.to_string()).into_response(),
    }
    match issue_worker_token(&s.pool, worker_id).await {
        Ok(token) => {
            let ev = AuditEvent::new(&actor, "intake.token", "issued").inputs(json!({ "worker_id": worker_id }));
            let _ = audit::append(&s.pool, &ev).await;
            reply(StatusCode::CREATED, json!({ "worker_id": worker_id, "token": token }))
        }
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}
