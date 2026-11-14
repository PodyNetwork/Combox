//! Admin dashboard endpoints. Every route requires a Supabase session with
//! TOTP MFA (`aal2`) belonging to an active row in `axom.admins`.
//! None of these are reachable through MCP.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_hr::approvals::AdminSession;
use axom_hr::cap;
use axom_hr::meetings::{self, Criterion};
use axom_hr::notify;
use axom_hr::policy::{self, Action, Context};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::meetings::meeting_error;
use crate::{admin, reply, AppState};

/// An MFA-verified, active admin, or the response to send.
pub(crate) async fn mfa_admin(s: &AppState, headers: &HeaderMap) -> Result<(AdminSession, Actor), Box<Response>> {
    let (_, session) = admin(s, headers).await?;
    if !session.mfa_verified {
        return Err(Box::new(reply(StatusCode::FORBIDDEN, json!({ "error": "MFA-verified session required" }))));
    }
    let row: Option<(String, bool)> = sqlx::query_as("SELECT name, active FROM axom.admins WHERE user_id = $1")
        .bind(session.user_id)
        .fetch_optional(&s.pool)
        .await
        .map_err(|e| Box::new(crate::ApiError(e.to_string()).into_response()))?;
    match row {
        Some((name, true)) => Ok((session, Actor::Admin { name, mfa_verified: true })),
        _ => Err(Box::new(reply(StatusCode::FORBIDDEN, json!({ "error": "not an active admin" })))),
    }
}

macro_rules! guard {
    ($s:expr, $h:expr) => {
        match mfa_admin(&$s, &$h).await {
            Ok(v) => v,
            Err(r) => return *r,
        }
    };
}

fn services(s: &AppState) -> Result<std::sync::Arc<crate::Services>, Box<Response>> {
    s.services.clone().ok_or_else(|| Box::new(reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "services not configured" }))))
}

fn internal(e: impl std::fmt::Display) -> Response {
    crate::ApiError(e.to_string()).into_response()
}

// ---------------------------------------------------------------------------
// Meetings
// ---------------------------------------------------------------------------

type MeetingRow = (Uuid, String, String, DateTime<Utc>, Vec<String>, String, Option<Value>, Option<Value>, Option<DateTime<Utc>>);

pub async fn list_meetings(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    let rows: Result<Vec<MeetingRow>, _> = sqlx::query_as(
        "SELECT id, title, room, scheduled_at, invitees, status, scores, consent, completed_at FROM axom.meetings ORDER BY scheduled_at DESC LIMIT 200",
    )
    .fetch_all(&s.pool)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<Value> = rows
                .into_iter()
                .map(|r| json!({ "id": r.0, "title": r.1, "room": r.2, "scheduled_at": r.3, "invitees": r.4, "status": r.5, "scores": r.6, "consent": r.7, "completed_at": r.8 }))
                .collect();
            reply(StatusCode::OK, json!({ "meetings": list }))
        }
        Err(e) => internal(e),
    }
}

pub async fn get_meeting(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    guard!(s, headers);
    let row: Result<Option<Value>, _> = sqlx::query_scalar(
        "SELECT to_jsonb(m) - 'callback_token_sha256' FROM axom.meetings m WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await;
    match row {
        Ok(Some(m)) => reply(StatusCode::OK, json!({ "meeting": m })),
        Ok(None) => reply(StatusCode::NOT_FOUND, json!({ "error": "meeting not found" })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct NewMeeting {
    pub title: String,
    pub scheduled_at: DateTime<Utc>,
    pub invitees: Vec<String>,
    #[serde(default)]
    pub candidate_worker_id: Option<Uuid>,
}

pub async fn create_meeting(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<NewMeeting>) -> Response {
    let (_, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match meetings::create_meeting(&s.pool, &svc.policy, svc.rooms.as_ref(), &actor, &b.title, b.scheduled_at, &b.invitees, b.candidate_worker_id).await {
        Ok(m) => {
            let links: Vec<Value> = m.invites.iter().map(|(e, t)| json!({ "email": e, "join_url": format!("{}/join?t={t}", svc.meet_web_url.trim_end_matches('/')) })).collect();
            let emailed = meetings::send_invites(&s.pool, &svc.policy, svc.mailer.as_ref(), &actor, m.meeting_id, &m.invites, &svc.meet_web_url, &svc.notify.owner_email).await;
            let emailed = match emailed {
                Ok(n) => json!(n),
                Err(e) => json!(format!("not sent: {e}")),
            };
            reply(StatusCode::CREATED, json!({ "meeting_id": m.meeting_id, "room": m.room, "join_links": links, "invites_emailed": emailed }))
        }
        Err(e) => meeting_error(e),
    }
}

/// A join link for the signed-in admin, to open the room from the dashboard.
pub async fn join_link(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let (session, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match meetings::admin_join_link(&s.pool, &actor, id, &session.email).await {
        Ok(t) => reply(StatusCode::OK, json!({ "meeting_id": id, "join_url": format!("{}/join?t={t}", svc.meet_web_url.trim_end_matches('/')) })),
        Err(e) => meeting_error(e),
    }
}

#[derive(Deserialize)]
pub struct Dispatch {
    #[serde(default)]
    pub instructions: String,
    /// The owner's own questions; optional (the interviewer writes its own).
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub rubric: Option<Vec<Criterion>>,
}

pub async fn dispatch(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<Dispatch>) -> Response {
    let (_, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match meetings::dispatch_agent(&s.pool, &svc.policy, svc.rooms.as_ref(), &actor, id, &b.instructions, &b.questions, b.rubric, &svc.public_api_url).await {
        // The callback token goes only to the agent (inside the dispatch metadata).
        Ok(d) => reply(StatusCode::OK, json!({ "meeting_id": id, "dispatch_id": d.dispatch_id, "agent": if d.dispatch_id.is_some() { "sent" } else { "joins when someone joins" } })),
        Err(e) => meeting_error(e),
    }
}

#[derive(Deserialize)]
pub struct Recipient {
    pub email: String,
}

pub async fn add_recipient(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<Recipient>) -> Response {
    let (_, actor) = guard!(s, headers);
    match meetings::add_recipient(&s.pool, &actor, id, &b.email).await {
        Ok(()) => reply(StatusCode::OK, json!({ "meeting_id": id, "added": b.email })),
        Err(e) => meeting_error(e),
    }
}

// ---------------------------------------------------------------------------
// Cap meter and cap changes
// ---------------------------------------------------------------------------

pub async fn cap_meter(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    let month = cap::month_of(Utc::now());
    let res = async {
        let u = cap::usage(&s.pool, month).await?;
        let (pending_count, pending_usd): (i64, Decimal) = sqlx::query_as(
            "SELECT count(*), COALESCE(SUM(l.amount_usd), 0) FROM axom.payouts p
             JOIN axom.cap_ledger l ON l.payout_id = p.id AND l.kind = 'reservation'
             WHERE p.status = 'pending_approval' AND l.month = $1",
        )
        .bind(month)
        .fetch_one(&s.pool)
        .await?;
        Ok::<_, anyhow::Error>(json!({
            "month": u.month, "cap_usd": u.cap_usd, "committed_usd": u.committed_usd, "reserved_usd": u.reserved_usd,
            "pending_approval_usd": pending_usd, "pending_approval_count": pending_count,
            "used_usd": u.used_usd(), "remaining_usd": u.remaining_usd(),
        }))
    }
    .await;
    match res {
        Ok(v) => reply(StatusCode::OK, v),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct CapChange {
    pub new_cap_usd: Decimal,
    pub reason: String,
}

pub async fn change_cap(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<CapChange>) -> Response {
    let (_, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if b.reason.trim().is_empty() {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "a reason is required" }));
    }
    match cap::change_cap(&s.pool, &svc.policy, &actor, b.new_cap_usd, &b.reason).await {
        Ok(()) => reply(StatusCode::OK, json!({ "cap_usd": b.new_cap_usd })),
        Err(cap::CapError::ChangeDenied(m)) => reply(StatusCode::FORBIDDEN, json!({ "error": m })),
        Err(e) => internal(e),
    }
}

// ---------------------------------------------------------------------------
// Notification feed
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct FeedQuery {
    #[serde(default)]
    pub kind: Option<String>,
}

pub async fn feed(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<FeedQuery>) -> Response {
    guard!(s, headers);
    match notify::feed(&s.pool, 200).await {
        Ok(items) => {
            let items: Vec<_> = items.into_iter().filter(|n| q.kind.as_deref().is_none_or(|k| n.kind == k)).collect();
            reply(StatusCode::OK, json!({ "notifications": items }))
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct AgentLogQuery {
    /// Only entries written or changed after this `rev`.
    #[serde(default)]
    pub after: Option<i64>,
}

/// The live agent log. Without `after`, the latest entries; with it, what
/// was written or changed since (poll with the highest `rev` you have).
pub async fn agent_log(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<AgentLogQuery>) -> Response {
    guard!(s, headers);
    let limit = if q.after.is_some() { 500 } else { 300 };
    match axom_planner::agent_log::since(&s.pool, q.after, limit).await {
        Ok(entries) => {
            let rev = entries.iter().map(|e| e.rev).max().or(q.after).unwrap_or(0);
            reply(StatusCode::OK, json!({ "entries": entries, "rev": rev }))
        }
        Err(e) => internal(e),
    }
}

/// Send a failed task back to the queue so a worker picks it up again.
/// Audited as this admin; resets the task's automatic retries.
pub async fn retry_task(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let (_, actor) = guard!(s, headers);
    use axom_planner::retry::{requeue, RetryError};
    match requeue(&s.pool, &actor, id, "retried from the dashboard", false).await {
        Ok(()) => reply(StatusCode::OK, json!({ "status": "queued" })),
        Err(RetryError::NotFound) => reply(StatusCode::NOT_FOUND, json!({ "error": "task not found" })),
        Err(e @ RetryError::NotFailed(_)) => reply(StatusCode::CONFLICT, json!({ "error": e.to_string() })),
        Err(RetryError::Db(e)) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct ReplyBody {
    pub text: String,
}

/// Answer an AI worker's questions on a task. The task goes back to the
/// worker, which carries on with the reply. Audited as this admin.
pub async fn reply_to_task(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(body): Json<ReplyBody>) -> Response {
    let (_, actor) = guard!(s, headers);
    use axom_planner::questions::{self, ReplyError};
    match questions::reply(&s.pool, &actor, id, &body.text).await {
        Ok(()) => reply(StatusCode::OK, json!({ "status": "sent" })),
        Err(ReplyError::NotFound) => reply(StatusCode::NOT_FOUND, json!({ "error": "task not found" })),
        Err(e @ ReplyError::NotWaiting) => reply(StatusCode::CONFLICT, json!({ "error": e.to_string() })),
        Err(e @ (ReplyError::Empty | ReplyError::TooLong)) => reply(StatusCode::BAD_REQUEST, json!({ "error": e.to_string() })),
        Err(ReplyError::Db(e)) => internal(e),
    }
}

// ---------------------------------------------------------------------------
// Kill switch
// ---------------------------------------------------------------------------

pub async fn kill_switch_state(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    let row: Result<(bool, DateTime<Utc>, String), _> =
        sqlx::query_as("SELECT kill_switch, updated_at, updated_by FROM axom.system_controls WHERE id = 1").fetch_one(&s.pool).await;
    match row {
        Ok((on, at, by)) => reply(StatusCode::OK, json!({ "on": on, "updated_at": at, "updated_by": by })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct KillSwitch {
    pub on: bool,
    #[serde(default)]
    pub reason: String,
}

/// One action that immediately halts (or resumes) all tool calls and
/// payouts. Every component reads the switch from the database before each
/// action, so it takes effect on the next check.
pub async fn set_kill_switch(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<KillSwitch>) -> Response {
    let (_, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let d = policy::evaluate(&svc.policy, &Action::ToggleKillSwitch { on: b.on }, &Context::new(actor.clone(), Utc::now()));
    if !d.is_allowed() {
        return reply(StatusCode::FORBIDDEN, json!({ "error": d.reason }));
    }
    let res = async {
        let mut tx = s.pool.begin().await?;
        sqlx::query("UPDATE axom.system_controls SET kill_switch = $1, updated_at = now(), updated_by = $2 WHERE id = 1")
            .bind(b.on)
            .bind(actor.to_string())
            .execute(&mut *tx)
            .await?;
        let ev = AuditEvent::new(&actor, "kill_switch.set", if b.on { "on" } else { "off" })
            .inputs(json!({ "on": b.on, "reason": b.reason }))
            .policy_version(&d.policy_version);
        let audit_id = audit::append(&mut *tx, &ev).await?;
        notify::enqueue(
            &mut *tx,
            "kill_switch",
            if b.on { "Kill switch ON: all tool calls and payouts halted" } else { "Kill switch OFF: operations resumed" },
            json!({ "on": b.on, "reason": b.reason, "by": actor.to_string(), "audit_link": svc.notify.audit_link(audit_id) }),
            Some(audit_id),
        )
        .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(())
    }
    .await;
    match res {
        Ok(()) => reply(StatusCode::OK, json!({ "on": b.on })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct DeletionRequest {
    pub reason: String,
}

/// Delete a candidate's data on request (owner, MFA-verified).
pub async fn delete_candidate(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<DeletionRequest>) -> Response {
    let (_, actor) = guard!(s, headers);
    let svc = match services(&s) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match axom_hr::privacy::delete_candidate_data(&s.pool, &svc.policy, &actor, id, &b.reason).await {
        Ok(()) => reply(StatusCode::OK, json!({ "candidate_id": id, "deleted": true })),
        Err(axom_hr::privacy::PrivacyError::NotFound(_)) => reply(StatusCode::NOT_FOUND, json!({ "error": "candidate not found" })),
        Err(axom_hr::privacy::PrivacyError::Denied(m)) => reply(StatusCode::CONFLICT, json!({ "error": m })),
        Err(e) => internal(e),
    }
}

/// Jobs-board applications, newest first, with where each candidate is in
/// hiring. What applicants wrote is untrusted and shown as plain text.
pub async fn list_applications(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    type Row = (Uuid, Uuid, String, String, String, Vec<String>, String, DateTime<Utc>, Option<Uuid>, Option<String>, Option<f64>, Option<f64>, Option<String>, Option<i32>);
    let rows: Result<Vec<Row>, _> = sqlx::query_as(
        "SELECT a.id, a.job_id, j.title, a.name, a.email, a.skills, a.message, a.created_at,
                c.id, c.status, c.shortlist_score, c.rubric_score, f.filename, f.size_bytes
         FROM axom.board_applications a
         JOIN axom.board_jobs j ON j.id = a.job_id
         LEFT JOIN axom.job_postings p ON p.marketplace = 'axom_board' AND p.external_job_id = a.job_id::text
         LEFT JOIN axom.candidates c ON c.job_posting_id = p.id AND c.external_applicant_id = a.id::text
         LEFT JOIN axom.board_application_cvs f ON f.application_id = a.id
         WHERE a.name <> '[deleted]'
         ORDER BY a.created_at DESC LIMIT 500",
    )
    .fetch_all(&s.pool)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<Value> = rows
                .into_iter()
                .map(|(id, job_id, job_title, name, email, skills, message, created_at, candidate_id, status, shortlist, rubric, cv_name, cv_size)| {
                    json!({
                        "id": id, "job_id": job_id, "job_title": job_title, "name": name, "email": email, "skills": skills,
                        "untrusted_message": message, "created_at": created_at, "candidate_id": candidate_id,
                        "status": status.unwrap_or_else(|| "applied".into()), "shortlist_score": shortlist, "interview_score": rubric,
                        "cv": cv_name.map(|n| json!({ "filename": n, "size_bytes": cv_size })),
                    })
                })
                .collect();
            reply(StatusCode::OK, json!({ "applications": list }))
        }
        Err(e) => internal(e),
    }
}

/// Download an applicant's CV (PDF). Served as an attachment that the
/// browser must not run or sniff; every download is audited.
pub async fn application_cv(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    use axum::http::header;
    let (_, actor) = guard!(s, headers);
    let row: Result<Option<(String, Vec<u8>)>, _> =
        sqlx::query_as("SELECT filename, content FROM axom.board_application_cvs WHERE application_id = $1").bind(id).fetch_optional(&s.pool).await;
    let (filename, content) = match row {
        Ok(Some(r)) => r,
        Ok(None) => return reply(StatusCode::NOT_FOUND, json!({ "error": "no CV for this application" })),
        Err(e) => return internal(e),
    };
    if let Err(e) = audit::append(&s.pool, &AuditEvent::new(&actor, "candidate.cv_download", "ok").inputs(json!({ "application_id": id }))).await {
        return internal(e);
    }
    let safe: String = filename.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')).collect();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{safe}\"")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        content,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Admin-defined tools
// ---------------------------------------------------------------------------

fn tool_error(e: axom_hr::custom_tools::CustomToolError) -> Response {
    use axom_hr::custom_tools::CustomToolError as E;
    match e {
        E::Invalid(m) => reply(StatusCode::BAD_REQUEST, json!({ "error": m })),
        E::Denied(m) => reply(StatusCode::FORBIDDEN, json!({ "error": m })),
        E::NotFound => reply(StatusCode::NOT_FOUND, json!({ "error": "not found" })),
        E::Tool(t) => {
            use axom_tools::ToolError as T;
            let (status, msg) = match t {
                T::Permanent(m) => (StatusCode::BAD_REQUEST, m),
                T::NotAuthorized(m) => (StatusCode::BAD_REQUEST, m),
                T::Retryable(m) => (StatusCode::BAD_GATEWAY, m),
                T::RateLimited { retry_after_secs } => (StatusCode::BAD_GATEWAY, format!("the tool is rate limiting; try again in {retry_after_secs}s")),
            };
            reply(status, json!({ "error": msg }))
        }
        E::Db(e) => internal(e),
    }
}

pub async fn list_tools(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    let listed = axom_hr::custom_tools::list(&s.pool).await;
    let available = axom_hr::custom_tools::available_tools(&s.pool).await;
    match (listed, available) {
        (Ok((servers, tools)), Ok(available)) => reply(
            StatusCode::OK,
            {
                // Standard tool names, and whether a server is connected under each.
                let standard: Vec<Value> = axom_hr::custom_tools::BUILTIN_TOOLS
                    .iter()
                    .map(|n| json!({ "name": n, "description": axom_tools::standard_tool_description(n), "connected": servers.iter().any(|s| s.name == *n && s.enabled) }))
                    .collect();
                json!({ "servers": servers, "tools": tools, "builtin": axom_hr::custom_tools::BUILTIN_TOOLS, "standard": standard, "available": available, "live": axom_tools::adapters::http::live_from_env() })
            },
        ),
        (Err(e), _) | (_, Err(e)) => internal(e),
    }
}

pub async fn create_tool_server(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<axom_hr::custom_tools::NewServer>) -> Response {
    let (_, actor) = guard!(s, headers);
    match axom_hr::custom_tools::create_server(&s.pool, s.secrets.as_ref(), &s.tool_guard, &actor, &b).await {
        Ok(id) => reply(StatusCode::CREATED, json!({ "server_id": id })),
        Err(e) => tool_error(e),
    }
}

pub async fn refresh_tool_server(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let (_, actor) = guard!(s, headers);
    match axom_hr::custom_tools::sync_server(&s.pool, s.secrets.as_ref(), &s.tool_guard, &actor, id).await {
        Ok(n) => reply(StatusCode::OK, json!({ "tools": n })),
        Err(e) => tool_error(e),
    }
}

#[derive(Deserialize)]
pub struct Enabled {
    pub enabled: bool,
}

pub async fn enable_tool_server(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<Enabled>) -> Response {
    let (_, actor) = guard!(s, headers);
    match axom_hr::custom_tools::set_server_enabled(&s.pool, &actor, id, b.enabled).await {
        Ok(()) => reply(StatusCode::OK, json!({ "enabled": b.enabled })),
        Err(e) => tool_error(e),
    }
}

pub async fn enable_tool(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<Enabled>) -> Response {
    let (_, actor) = guard!(s, headers);
    match axom_hr::custom_tools::set_tool_enabled(&s.pool, &actor, id, b.enabled).await {
        Ok(()) => reply(StatusCode::OK, json!({ "enabled": b.enabled })),
        Err(e) => tool_error(e),
    }
}

pub async fn delete_tool_server(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let (_, actor) = guard!(s, headers);
    match axom_hr::custom_tools::delete_server(&s.pool, &actor, id).await {
        Ok(()) => reply(StatusCode::OK, json!({ "deleted": id })),
        Err(e) => tool_error(e),
    }
}

#[derive(Deserialize)]
pub struct WorkerTools {
    pub tools: Vec<String>,
}

pub async fn set_worker_tools(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<WorkerTools>) -> Response {
    let (_, actor) = guard!(s, headers);
    let Some(cfg) = s.approvals.as_ref().map(|a| a.policy.clone()) else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "approvals are not configured" }));
    };
    match axom_hr::custom_tools::set_worker_tools(&s.pool, &cfg, &actor, id, &b.tools).await {
        Ok(tools) => reply(StatusCode::OK, json!({ "worker_id": id, "tools": tools })),
        Err(e) => tool_error(e),
    }
}

// ---------------------------------------------------------------------------
// Company rules (stricter than the platform's only)
// ---------------------------------------------------------------------------

fn base_policy(s: &AppState) -> Result<std::sync::Arc<axom_hr::PolicyConfig>, Box<Response>> {
    s.approvals.as_ref().map(|a| a.policy.clone()).ok_or_else(|| Box::new(reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "approvals are not configured" }))))
}

pub async fn company_policy(State(s): State<AppState>, headers: HeaderMap) -> Response {
    guard!(s, headers);
    let base = match base_policy(&s) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    match axom_hr::company_policy::load(&s.pool).await {
        Ok((rules, changed)) => {
            let eff = axom_hr::company_policy::apply(&base, &rules);
            reply(
                StatusCode::OK,
                json!({
                    "platform": { "approval_threshold_usd": base.payouts().approval_threshold_usd, "candidate_retention_days": base.file.privacy.candidate_retention_days },
                    "company": rules,
                    "effective": { "approval_threshold_usd": eff.payouts().approval_threshold_usd, "candidate_retention_days": eff.file.privacy.candidate_retention_days, "version": eff.version() },
                    "updated_by": changed.as_ref().map(|c| c.0.clone()),
                    "updated_at": changed.map(|c| c.1),
                }),
            )
        }
        Err(e) => internal(e),
    }
}

pub async fn set_company_policy(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<axom_hr::company_policy::CompanyRules>) -> Response {
    let (_, actor) = guard!(s, headers);
    let base = match base_policy(&s) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    use axom_hr::company_policy::CompanyRulesError as E;
    match axom_hr::company_policy::set(&s.pool, &base, &actor, &b).await {
        Ok(eff) => reply(StatusCode::OK, json!({ "effective": { "approval_threshold_usd": eff.payouts().approval_threshold_usd, "candidate_retention_days": eff.file.privacy.candidate_retention_days } })),
        Err(E::Denied(m)) => reply(StatusCode::FORBIDDEN, json!({ "error": m })),
        Err(E::Invalid(m)) => reply(StatusCode::BAD_REQUEST, json!({ "error": m })),
        Err(E::Db(e)) => internal(e),
    }
}
