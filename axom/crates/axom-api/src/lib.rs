//! HTTP API. Read-only views so far: health, org chart, tasks, cap usage,
//! the owner notification feed, and spot-check stats. Approval endpoints (MFA-only, never via MCP) arrive in
//! milestone 4 and the task intake queue in milestone 5.

use axom_core::db;
use axom_core::state::TaskStatus;
use axom_hr::{cap, notify, reviews};
use axom_planner::store;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

use axom_core::authz::AuthorizationKey;
use axom_hr::approvals::{self, AdminSession, ApprovalError};
use axom_hr::payouts::PayoutError;
use axom_hr::PolicyConfig;
use sqlx::PgPool;
use uuid::Uuid;

pub mod admin;
pub mod auth;
pub mod integrations;
pub mod destinations;
pub mod intake;
pub mod jobs;
pub mod mcp;
pub mod meetings;
pub mod treasury;

/// Dashboard approval configuration. Without it, approval endpoints return 503.
#[derive(Clone)]
pub struct ApprovalConfig {
    pub auth: auth::AuthConfig,
    pub policy: Arc<PolicyConfig>,
    /// Signs approval records for the signer (`AXOM_APPROVAL_SIGNING_KEY`).
    pub approval_key: Arc<AuthorizationKey>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub approvals: Option<ApprovalConfig>,
    /// Service-to-service token for internal callers (`AXOM_INTERNAL_TOKEN`).
    pub internal_token: Option<String>,
    /// External services for meetings, summaries, and the MCP server.
    pub services: Option<Arc<Services>>,
    /// Bearer token for the MCP server (`AXOM_MCP_TOKEN`, at least 32 chars).
    pub mcp_token: Option<String>,
    /// Encrypts custom tool tokens (`AXOM_SECRETS_KEY`).
    pub secrets: Option<axom_core::secretbox::SecretBox>,
    /// Where admin-defined tools may connect.
    pub tool_guard: axom_tools::adapters::mcp::UrlGuard,
    /// Single-company mode: integration keys are set at `/admin/integrations`
    /// (the platform stores them itself, so it leaves this off).
    pub local_integrations: bool,
    /// Bumped when a setting is saved, so the server rebuilds its adapters.
    pub settings_generation: Arc<std::sync::atomic::AtomicU64>,
    /// Single-company mode (`AXOM_OWNER_EMAIL`): while there are no admins,
    /// this person becomes the first one on their first two-factor sign-in.
    /// The platform leaves it unset: a company's creator is its owner there.
    pub owner_email: Option<String>,
    /// The company's NEAR treasury, shown read-only on the dashboard.
    pub treasury: treasury::TreasuryConfig,
}

/// Adapters and settings the meeting and MCP endpoints use.
pub struct Services {
    pub policy: Arc<PolicyConfig>,
    pub notify: axom_hr::notify::NotifyConfig,
    pub rooms: Arc<dyn axom_tools::MeetingRooms>,
    pub mailer: Arc<dyn axom_tools::Mailer>,
    pub crm: Arc<dyn axom_tools::Crm>,
    /// `wss://` URL participants connect to.
    pub livekit_ws_url: String,
    /// Base URL the meeting agent calls back on (`AXOM_AGENT_CALLBACK_URL`,
    /// else the public API URL).
    pub public_api_url: String,
    /// Public base URL of the participant UI (join links).
    pub meet_web_url: String,
    /// The company's own interview AI keys (Deepgram, Cartesia, Anthropic),
    /// handed to the interview agent for its meetings. Unset keys are left
    /// out and the agent uses its own.
    pub agent_keys: std::collections::HashMap<String, String>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            approvals: None,
            internal_token: std::env::var("AXOM_INTERNAL_TOKEN").ok().filter(|t| t.len() >= 32),
            services: None,
            mcp_token: std::env::var("AXOM_MCP_TOKEN").ok().filter(|t| t.len() >= 32),
            secrets: axom_core::secretbox::SecretBox::from_env().ok(),
            tool_guard: axom_tools::adapters::mcp::UrlGuard::from_env(),
            local_integrations: false,
            settings_generation: Arc::default(),
            owner_email: None,
            treasury: treasury::TreasuryConfig::from_env(),
        }
    }
}

fn board_cv_limit() -> usize {
    axom_hr::board::MAX_CV_BYTES + 64 * 1024
}

pub fn router(state: AppState) -> Router {
    routes(state).layer(cors())
}

/// The API routes without CORS, for embedding in a larger router that
/// applies CORS once (the multi-tenant platform).
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/org-chart", get(org_chart))
        .route("/tasks", get(tasks))
        .route("/cap", get(cap_usage))
        .route("/notifications", get(notifications))
        .route("/spot-checks/stats", get(spot_checks))
        // Approvals: dashboard only, named admin with an MFA-verified
        // session. Never exposed through MCP or to agents.
        .route("/payouts/pending", get(pending_payouts))
        .route("/payouts/{id}/approve", post(approve))
        .route("/payouts/{id}/reject", post(reject))
        .route("/payouts/cross-chain", get(destinations::list))
        .route("/workers/me/payout-destination", get(destinations::get_mine).delete(destinations::revoke))
        .route("/workers/me/payout-destination/challenge", post(destinations::challenge))
        .route("/workers/me/payout-destination/verify", post(destinations::verify))
        .route("/workers/me/payout-address/challenge", post(destinations::address_challenge))
        .route("/workers/me/payout-address/verify", post(destinations::address_verify))
        // Task intake (internal queue) and freelancer deliverables.
        .route("/intake/tasks", post(intake::submit_task))
        .route("/intake/briefs", get(intake::my_briefs))
        .route("/intake/briefs/{task_id}/deliverables", post(intake::submit_deliverable))
        .route("/intake/briefs/{task_id}/review", post(intake::review_deliverable))
        .route("/workers/{id}/intake-token", post(intake::new_worker_token))
        // Meetings: participant join links and the meeting agent's callback.
        // Public jobs board (Combox Jobs): no sign-in.
        .route("/jobs", get(jobs::list))
        .route("/jobs/{id}", get(jobs::get))
        // Room for a 5 MB CV plus the form.
        .route("/jobs/{id}/apply", post(jobs::apply).layer(axum::extract::DefaultBodyLimit::max(board_cv_limit())))
        .route("/meetings/join", post(meetings::join))
        .route("/meetings/{id}/callback", get(meetings::agent_keys).post(meetings::callback))
        // Admin dashboard (MFA-verified admins only).
        .route("/admin/meetings", get(admin::list_meetings).post(admin::create_meeting))
        .route("/admin/meetings/{id}", get(admin::get_meeting))
        .route("/admin/meetings/{id}/dispatch", post(admin::dispatch))
        .route("/admin/meetings/{id}/join-link", post(admin::join_link))
        .route("/admin/meetings/{id}/recipients", post(admin::add_recipient))
        .route("/admin/treasury", get(treasury::treasury))
        .route("/admin/treasury/yield", get(treasury::treasury_yield))
        .route("/admin/cap", get(admin::cap_meter).post(admin::change_cap))
        .route("/admin/notifications", get(admin::feed))
        .route("/admin/agent-log", get(admin::agent_log))
        .route("/admin/tasks/{id}/retry", post(admin::retry_task))
        .route("/admin/tasks/{id}/start-now", post(admin::start_task_now))
        .route("/admin/tasks/{id}/reply", post(admin::reply_to_task))
        .route("/admin/kill-switch", get(admin::kill_switch_state).post(admin::set_kill_switch))
        .route("/admin/candidates/{id}/delete", post(admin::delete_candidate))
        .route("/admin/applications", get(admin::list_applications))
        .route("/admin/applications/{id}/cv", get(admin::application_cv))
        // Admin-defined tools (MCP servers, webhooks) and worker allowlists.
        .route("/admin/tools", get(admin::list_tools).post(admin::create_tool_server))
        .route("/admin/tools/{id}", axum::routing::delete(admin::delete_tool_server))
        .route("/admin/tools/{id}/refresh", post(admin::refresh_tool_server))
        .route("/admin/tools/{id}/enabled", post(admin::enable_tool_server))
        .route("/admin/tools/items/{id}/enabled", post(admin::enable_tool))
        .route("/admin/workers/{id}/tools", post(admin::set_worker_tools))
        // The company's own, stricter payout and privacy rules.
        .route("/admin/policy", get(admin::company_policy).post(admin::set_company_policy))
        // Integration settings and API keys (single-company mode).
        .route("/admin/integrations", get(integrations::list))
        .route("/admin/integrations/{name}", axum::routing::put(integrations::set).delete(integrations::delete))
        // MCP server: meeting tools only.
        .route("/mcp", post(mcp::handle))
        .with_state(state)
}

/// CORS for the participant UI and the admin dashboard
/// (`AXOM_CORS_ORIGINS`, comma-separated; default: local dev servers).
pub fn cors() -> tower_http::cors::CorsLayer {
    use axum::http::{header, HeaderValue, Method};
    let origins: Vec<HeaderValue> = std::env::var("AXOM_CORS_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:5173,http://localhost:5174,http://localhost:4173".into())
        .split(',')
        .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
        .collect();
    tower_http::cors::CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

pub(crate) struct ApiError(pub(crate) String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "api error");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal error" }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.to_string())
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn org_chart(State(s): State<AppState>) -> Result<Response, ApiError> {
    let Some(plan_id) = store::current_plan_id(&s.pool).await? else {
        return Ok((StatusCode::NOT_FOUND, Json(json!({ "error": "no plan loaded" }))).into_response());
    };
    let chart = store::load_org_chart(&s.pool, plan_id).await?;
    Ok(Json(json!({ "plan_id": plan_id, "departments": chart })).into_response())
}

async fn tasks(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let mut all = Vec::new();
    for status in TaskStatus::ALL {
        all.extend(db::tasks_with_status(&s.pool, *status).await?);
    }
    // Failed tasks carry their next automatic retry, if one is planned.
    let planned = axom_planner::retry::planned(&s.pool).await?;
    let tasks: Vec<serde_json::Value> = all
        .into_iter()
        .map(|t| {
            let auto = planned.get(&t.id).cloned();
            let mut v = json!(t);
            if let Some(a) = auto {
                v["auto_retry"] = json!(a);
            }
            v
        })
        .collect();
    Ok(Json(json!({ "tasks": tasks })))
}

async fn cap_usage(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let u = cap::usage(&s.pool, cap::month_of(Utc::now())).await?;
    Ok(Json(json!({
        "month": u.month, "cap_usd": u.cap_usd, "reserved_usd": u.reserved_usd,
        "committed_usd": u.committed_usd, "used_usd": u.used_usd(), "remaining_usd": u.remaining_usd(),
    })))
}

async fn notifications(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!({ "notifications": notify::feed(&s.pool, 100).await? })))
}

async fn spot_checks(State(s): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!(reviews::spot_check_stats(&s.pool).await?)))
}

pub(crate) fn reply(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

/// Authenticate the request. Returns the approval config and session, or the
/// error response to send.
pub(crate) async fn admin(s: &AppState, headers: &HeaderMap) -> Result<(ApprovalConfig, AdminSession), Box<Response>> {
    let Some(cfg) = s.approvals.clone() else {
        return Err(Box::new(reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "approvals are not configured" }))));
    };
    let header = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    match auth::session_from_bearer(&cfg.auth, header).await {
        Ok(session) => {
            bootstrap_owner(s, &session).await.map_err(|e| Box::new(ApiError(e.to_string()).into_response()))?;
            Ok((cfg, session))
        }
        Err(e) => Err(Box::new(reply(StatusCode::UNAUTHORIZED, json!({ "error": e.to_string() })))),
    }
}

/// Single-company setup without SQL: the first two-factor (aal2) sign-in by
/// `AXOM_OWNER_EMAIL` makes that user the first admin. Only while
/// `axom.admins` is completely empty, so once anyone is an admin (or the
/// owner was deactivated) this never adds anyone. Audited.
async fn bootstrap_owner(s: &AppState, session: &AdminSession) -> Result<(), sqlx::Error> {
    let Some(owner) = s.owner_email.as_deref().map(str::trim).filter(|e| !e.is_empty()) else { return Ok(()) };
    if !session.mfa_verified || !session.email.trim().eq_ignore_ascii_case(owner) {
        return Ok(());
    }
    let added = sqlx::query(
        "INSERT INTO axom.admins (user_id, email, name) SELECT $1, $2, 'Owner' \
         WHERE NOT EXISTS (SELECT 1 FROM axom.admins) ON CONFLICT DO NOTHING",
    )
    .bind(session.user_id)
    .bind(session.email.trim())
    .execute(&s.pool)
    .await?
    .rows_affected();
    if added == 1 {
        let actor = axom_core::domain::Actor::Admin { name: "Owner".into(), mfa_verified: true };
        let ev = axom_core::audit::AuditEvent::new(&actor, "admin.bootstrap_owner", "added")
            .inputs(json!({ "user_id": session.user_id, "email": session.email, "session_id": session.session_id }))
            .reasoning("first two-factor sign-in by AXOM_OWNER_EMAIL while no admins existed");
        axom_core::audit::append(&s.pool, &ev).await?;
        tracing::info!(user_id = %session.user_id, "owner signed in for the first time and is now an admin");
    }
    Ok(())
}

type PendingRow = (
    Uuid,
    Option<Uuid>,
    String,
    rust_decimal::Decimal,
    Option<String>,
    Option<rust_decimal::Decimal>,
    String,
    chrono::DateTime<Utc>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn approval_error(e: ApprovalError) -> Response {
    match e {
        ApprovalError::MfaRequired | ApprovalError::NotAdmin => reply(StatusCode::FORBIDDEN, json!({ "error": e.to_string() })),
        ApprovalError::NoAmount | ApprovalError::Payout(PayoutError::Transition(_)) | ApprovalError::Payout(PayoutError::Denied(_)) => {
            reply(StatusCode::CONFLICT, json!({ "error": e.to_string() }))
        }
        ApprovalError::Payout(PayoutError::Unauthorized(_)) => reply(StatusCode::FORBIDDEN, json!({ "error": e.to_string() })),
        other => ApiError(other.to_string()).into_response(),
    }
}

async fn pending_payouts(State(s): State<AppState>, headers: HeaderMap) -> Response {
    // Recipients and amounts: MFA-verified admins of this company only.
    if let Err(r) = admin::mfa_admin(&s, &headers).await {
        return *r;
    }
    let rows: Result<Vec<PendingRow>, _> = sqlx::query_as(
        "SELECT id, worker_id, recipient, amount_usd, amount_yocto::text, usd_per_near, status_reason, created_at,
                dest_chain, dest_asset, dest_address
         FROM axom.payouts WHERE status = 'pending_approval' ORDER BY created_at",
    )
    .fetch_all(&s.pool)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    // Cross-chain payouts: the money lands at `destination`; `recipient`
                    // is the worker's verified NEAR account that signed it.
                    let destination = r.8.as_ref().map(|c| {
                        json!({ "chain": c, "chain_name": axom_core::intents::chain_name(c), "asset_id": r.9, "address": r.10, "via": "NEAR Intents" })
                    });
                    json!({ "id": r.0, "worker_id": r.1, "recipient": r.2, "amount_usd": r.3, "amount_yocto": r.4,
                            "usd_per_near": r.5, "reason": r.6, "created_at": r.7, "destination": destination })
                })
                .collect();
            reply(StatusCode::OK, json!({ "payouts": list }))
        }
        Err(e) => ApiError(e.to_string()).into_response(),
    }
}

async fn approve(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let (cfg, session) = match admin(&s, &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match approvals::approve_payout(&s.pool, &cfg.policy, &cfg.approval_key, &session, id, Utc::now()).await {
        Ok(approval_id) => reply(StatusCode::OK, json!({ "payout_id": id, "approval_id": approval_id, "status": "approved" })),
        Err(e) => approval_error(e),
    }
}

#[derive(serde::Deserialize)]
struct RejectBody {
    reason: String,
}

async fn reject(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(body): Json<RejectBody>) -> Response {
    let (cfg, session) = match admin(&s, &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match approvals::reject_payout(&s.pool, &cfg.policy, &session, id, &body.reason, Utc::now()).await {
        Ok(approval_id) => reply(StatusCode::OK, json!({ "payout_id": id, "approval_id": approval_id, "status": "rejected" })),
        Err(e) => approval_error(e),
    }
}
