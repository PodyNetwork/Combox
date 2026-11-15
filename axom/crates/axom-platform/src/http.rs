//! Platform HTTP API and per-company routing.
//!
//! - `/platform/...`: sign-up, companies, members, integrations, billing.
//! - `/t/<company>/...`: that company's own Combox API (dashboard, approvals,
//!   jobs board, meetings, MCP), served against its own database. The path
//!   prefix is stripped and the request handed to a router built for that
//!   company, so every existing endpoint works per company unchanged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axom_api::auth::{self, AuthConfig};
use axom_api::{AppState, ApprovalConfig, Services};
use axom_core::authz::AuthorizationKey;
use axom_hr::approvals::AdminSession;
use axom_hr::notify::NotifyConfig;
use axom_tools::config::{ConfigSource, EnvSource, Layered};
use axom_tools::mock::MockTools;
use axom_tools::price::PriceFeed;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use crate::billing::{PaymentError, PaymentLookup};
use crate::secrets::{CATALOG, SHARED_KEYS, SHARED_STANDALONE};
use axom_tools::integrations::INTERVIEW_KEYS;
use crate::tenants::{Platform, Subscription, Tenant, TenantError};

/// Read-only views that a single-company deployment serves without sign-in
/// (on its private network). On the platform they hold one company's data,
/// so only that company's signed-in, MFA-verified members may read them.
const MEMBERS_ONLY: &[&str] = &["org-chart", "tasks", "cap", "notifications", "spot-checks"];

/// How long a company's router (with its adapters) is reused before being
/// rebuilt; changes to its integration keys rebuild it immediately.
const ROUTER_TTL: Duration = Duration::from_secs(300);

pub struct PlatformState {
    pub platform: Arc<Platform>,
    pub auth: AuthConfig,
    pub approval_key: Arc<AuthorizationKey>,
    pub price: Arc<dyn PriceFeed>,
    pub payments: Arc<dyn PaymentLookup>,
    /// Public base URL of this API, e.g. `https://api.axom.example`.
    pub public_api_url: String,
    /// Base URL the meeting agent calls back on; the public URL unless set
    /// (see `with_agent_callback_url`).
    pub agent_callback_url: String,
    /// Public base URL of the candidate site (jobs board, interviews).
    pub meet_web_url: String,
    pub dashboard_url: String,
    /// Planner settings for companies' plans (org chart from goals).
    pub planner: Option<Arc<axom_planner::PlannerConfig>>,
    /// Creates companies' treasury keys (OpenBao provisioner), if configured.
    pub treasury: Option<Arc<dyn crate::treasury::TreasuryKeys>>,
    /// Sends invitation emails, if configured.
    pub mail: Option<Arc<crate::mail::PlatformMail>>,
    /// Who may see every company and suspend them (`AXOM_PLATFORM_OPERATORS`).
    pub operators: crate::operator::Operators,
    routers: RwLock<HashMap<Uuid, (Router, Instant)>>,
}

impl PlatformState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        platform: Arc<Platform>,
        auth: AuthConfig,
        approval_key: Arc<AuthorizationKey>,
        price: Arc<dyn PriceFeed>,
        payments: Arc<dyn PaymentLookup>,
        public_api_url: String,
        meet_web_url: String,
        dashboard_url: String,
    ) -> Self {
        Self { platform, auth, approval_key, price, payments, agent_callback_url: public_api_url.clone(), public_api_url, meet_web_url, dashboard_url, planner: None, treasury: None, mail: None, operators: Default::default(), routers: RwLock::default() }
    }

    pub fn with_planner(mut self, planner: Arc<axom_planner::PlannerConfig>) -> Self {
        self.planner = Some(planner);
        self
    }

    /// Where the meeting agent reaches this API, when that differs from the
    /// public URL (for example a Docker service name).
    pub fn with_agent_callback_url(mut self, url: String) -> Self {
        self.agent_callback_url = url;
        self
    }

    pub fn with_operators(mut self, operators: crate::operator::Operators) -> Self {
        self.operators = operators;
        self
    }

    pub fn with_mail(mut self, mail: Arc<crate::mail::PlatformMail>) -> Self {
        self.mail = Some(mail);
        self
    }

    pub fn with_treasury(mut self, keys: Arc<dyn crate::treasury::TreasuryKeys>) -> Self {
        self.treasury = Some(keys);
        self
    }

    pub async fn invalidate(&self, tenant: Uuid) {
        self.routers.write().await.remove(&tenant);
    }

    /// Services for a company: its own adapters from its stored keys, with
    /// the platform's shared email and video as fallback; mocks otherwise
    /// (dry-run unless AXOM_LIVE=true).
    pub async fn services(&self, t: &Tenant) -> anyhow::Result<Services> {
        use axom_tools::adapters::{livekit::LiveKit, smtp::SmtpMailer, opendesk::OpendeskCrm};
        let own = self.platform.config(t.id).await?;
        let cfg = Layered { own: &own, fallback: &EnvSource, shared_keys: SHARED_KEYS, standalone_keys: SHARED_STANDALONE };
        let mock = Arc::new(MockTools::new());
        // In live mode a service that can't be built refuses every call; a
        // mock there would report invites and emails as sent.
        let live = axom_tools::adapters::http::live_from_env();
        let not_set_up = |service: &'static str, e: &anyhow::Error| {
            tracing::warn!(tenant = %t.id, service, error = %e, "live mode is on but this service is not set up; its actions will fail until it is");
            Arc::new(axom_tools::not_set_up::NotSetUp::new(service, e))
        };
        let rooms: Arc<dyn axom_tools::MeetingRooms> = match LiveKit::from_env() {
            Ok(l) => Arc::new(l),
            Err(e) if live => not_set_up("video meetings (LiveKit)", &e),
            Err(_) => mock.clone(),
        };
        let mailer: Arc<dyn axom_tools::Mailer> = match SmtpMailer::from_config(&cfg) {
            Ok(m) => Arc::new(m),
            Err(e) if live => not_set_up("email (SMTP)", &e),
            Err(_) => mock.clone(),
        };
        let crm: Arc<dyn axom_tools::Crm> = match OpendeskCrm::from_config(&cfg) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                if cfg.get("CRM_API_KEY").is_some() || cfg.get("CRM_URL").is_some() {
                    tracing::warn!(tenant = %t.id, error = %e, "CRM (opendesk) settings can't be used; meeting summaries won't reach the CRM");
                }
                mock.clone()
            }
        };
        Ok(Services {
            policy: self.platform.policy.clone(),
            notify: NotifyConfig { owner_email: t.owner_email.clone(), dashboard_url: format!("{}/c/{}", self.dashboard_url.trim_end_matches('/'), t.slug) },
            rooms,
            mailer,
            crm,
            livekit_ws_url: std::env::var("LIVEKIT_URL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "ws://localhost:7880".into()),
            public_api_url: tenant_url(&self.agent_callback_url, &t.slug),
            meet_web_url: tenant_url(&self.meet_web_url, &t.slug),
            agent_keys: INTERVIEW_KEYS.iter().filter_map(|k| own.get(k).map(|v| (k.to_string(), v))).collect(),
        })
    }

    async fn router_for(&self, t: &Tenant) -> anyhow::Result<Router> {
        if let Some((r, at)) = self.routers.read().await.get(&t.id) {
            if at.elapsed() < ROUTER_TTL {
                return Ok(r.clone());
            }
        }
        let own = self.platform.config(t.id).await?;
        let state = AppState {
            pool: self.platform.pool(t).await,
            approvals: Some(ApprovalConfig { auth: self.auth.clone(), policy: self.platform.policy.clone(), approval_key: self.approval_key.clone() }),
            internal_token: None,
            services: Some(Arc::new(self.services(t).await?)),
            mcp_token: own.get("AXOM_MCP_TOKEN"),
            secrets: Some(self.platform.secrets.clone()),
            // Companies' tools may only reach the public internet.
            tool_guard: axom_tools::adapters::mcp::UrlGuard::strict(),
            local_integrations: false,
            settings_generation: Arc::default(),
            // A company's creator is its owner; no bootstrap by email here.
            owner_email: None,
            // Its own treasury, if created (never the signer's single key).
            treasury: axom_api::treasury::TreasuryConfig::with_source(t.treasury_account.clone().map(axom_api::treasury::TreasurySource::Account)),
        };
        let r = axom_api::routes(state);
        self.routers.write().await.insert(t.id, (r.clone(), Instant::now()));
        Ok(r)
    }
}

/// `https://api.example.com` + `acme` -> `https://api.example.com/t/acme`.
pub fn tenant_url(base: &str, slug: &str) -> String {
    format!("{}/t/{slug}", base.trim_end_matches('/'))
}

type S = State<Arc<PlatformState>>;

pub fn router(state: Arc<PlatformState>) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok", "mode": "multi-tenant" })) }))
        .route("/platform/plan", get(plan))
        .route("/platform/integrations", get(|| async { Json(json!({ "integrations": CATALOG })) }))
        .route("/platform/me", get(me))
        .route("/platform/operator/companies", get(operator_companies))
        .route("/platform/operator/companies/{slug}/suspend", post(operator_suspend))
        .route("/platform/operator/companies/{slug}/resume", post(operator_resume))
        .route("/platform/companies", post(create_company))
        .route("/platform/companies/{slug}", get(company))
        .route("/platform/companies/{slug}/integrations", get(list_integrations))
        .route("/platform/companies/{slug}/integrations/{name}", put(set_integration).delete(delete_integration))
        .route("/platform/companies/{slug}/members", get(members))
        .route("/platform/companies/{slug}/members/{user}", delete(remove_member))
        .route("/platform/companies/{slug}/invites", post(invite))
        .route("/platform/companies/{slug}/invites/{id}", delete(revoke_invite))
        .route("/platform/companies/{slug}/treasury", post(setup_treasury))
        .route("/platform/companies/{slug}/plan", post(set_plan))
        .route("/platform/companies/{slug}/billing", get(billing))
        .route("/platform/companies/{slug}/billing/invoices", post(create_invoice))
        .route("/platform/companies/{slug}/billing/invoices/{id}/verify", post(verify_invoice))
        .route("/t/{slug}", any(tenant_dispatch_root))
        .route("/t/{slug}/{*rest}", any(tenant_dispatch))
        .layer(axom_api::cors())
        .with_state(state)
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

fn tenant_error(e: TenantError) -> Response {
    let status = match &e {
        TenantError::Invalid(_) => StatusCode::BAD_REQUEST,
        TenantError::SlugTaken => StatusCode::CONFLICT,
        TenantError::Forbidden(_) => StatusCode::FORBIDDEN,
        TenantError::NotFound => StatusCode::NOT_FOUND,
        TenantError::Provision(_) | TenantError::Db(_) => {
            tracing::error!(error = %e, "platform error");
            return reply(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "something went wrong; please try again" }));
        }
    };
    reply(status, json!({ "error": e.to_string() }))
}

async fn session(ps: &PlatformState, headers: &HeaderMap) -> Result<AdminSession, Box<Response>> {
    let h = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    auth::session_from_bearer(&ps.auth, h).await.map_err(|e| Box::new(reply(e.status(), json!({ "error": e.to_string() }))))
}

/// The company and the caller's role, for a signed-in member.
async fn member(ps: &PlatformState, headers: &HeaderMap, slug: &str) -> Result<(Tenant, AdminSession, String), Box<Response>> {
    let s = session(ps, headers).await?;
    let t = ps.platform.by_slug(slug).await.map_err(|e| Box::new(tenant_error(e)))?;
    match ps.platform.role(t.id, s.user_id).await {
        Ok(Some(role)) => Ok((t, s, role)),
        Ok(None) => Err(Box::new(reply(StatusCode::NOT_FOUND, json!({ "error": "company not found" })))),
        Err(e) => Err(Box::new(tenant_error(e.into()))),
    }
}

macro_rules! member {
    ($ps:expr, $h:expr, $slug:expr) => {
        match member(&$ps, &$h, &$slug).await {
            Ok(v) => v,
            Err(r) => return *r,
        }
    };
}

async fn plan(State(ps): S) -> Response {
    let p = &ps.platform.plan;
    reply(StatusCode::OK, json!({ "name": p.name, "price_usd": p.price_usd, "trial_days": p.trial_days, "network": p.network, "payments_enabled": p.pay_to.is_some() }))
}

fn company_json(t: &Tenant, role: &str) -> Value {
    json!({ "company": t, "role": role, "subscription": t.subscription(Utc::now()) })
}

async fn me(State(ps): S, headers: HeaderMap) -> Response {
    let s = match session(&ps, &headers).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match ps.platform.memberships(&s).await {
        Ok(list) => reply(
            StatusCode::OK,
            json!({
                "user": { "id": s.user_id, "email": s.email, "mfa_verified": s.mfa_verified },
                "operator": ps.operators.allows(&s),
                "companies": list.iter().map(|(t, r)| company_json(t, r)).collect::<Vec<_>>(),
            }),
        ),
        Err(e) => tenant_error(e),
    }
}

#[derive(Deserialize)]
struct NewCompany {
    name: String,
    slug: String,
}

async fn create_company(State(ps): S, headers: HeaderMap, Json(b): Json<NewCompany>) -> Response {
    let s = match session(&ps, &headers).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match ps.platform.create_tenant(&s, &b.name, &b.slug, Utc::now()).await {
        Ok(t) => reply(StatusCode::CREATED, company_json(&t, "owner")),
        Err(e) => tenant_error(e),
    }
}

async fn company(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let (t, _, role) = member!(ps, headers, slug);
    let mut v = company_json(&t, &role);
    let pool = ps.platform.pool(&t).await;
    let has_plan: Result<(bool,), _> = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM axom.plans)").fetch_one(&pool).await;
    v["has_plan"] = json!(has_plan.map(|r| r.0).unwrap_or(false));
    v["treasury_available"] = json!(ps.treasury.is_some());
    v["urls"] = json!({
        "api": tenant_url(&ps.public_api_url, &t.slug),
        "jobs_board": format!("{}/jobs", tenant_url(&ps.meet_web_url, &t.slug)),
        "mcp": format!("{}/mcp", tenant_url(&ps.public_api_url, &t.slug)),
    });
    reply(StatusCode::OK, v)
}

async fn list_integrations(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.list_secrets(t.id, &s).await {
        Ok(values) => reply(StatusCode::OK, json!({ "integrations": CATALOG, "values": values, "shared": SHARED_KEYS, "live": axom_tools::adapters::http::live_from_env() })),
        Err(e) => tenant_error(e),
    }
}

#[derive(Deserialize)]
struct SetValue {
    value: String,
}

async fn set_integration(State(ps): S, headers: HeaderMap, Path((slug, name)): Path<(String, String)>, Json(b): Json<SetValue>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.set_secret(t.id, &s, &name, &b.value).await {
        Ok(()) => {
            ps.invalidate(t.id).await;
            reply(StatusCode::OK, json!({ "saved": name }))
        }
        Err(e) => tenant_error(e),
    }
}

async fn delete_integration(State(ps): S, headers: HeaderMap, Path((slug, name)): Path<(String, String)>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.delete_secret(t.id, &s, &name).await {
        Ok(()) => {
            ps.invalidate(t.id).await;
            reply(StatusCode::OK, json!({ "deleted": name }))
        }
        Err(e) => tenant_error(e),
    }
}

async fn members(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let (t, _, role) = member!(ps, headers, slug);
    match ps.platform.members(t.id).await {
        Ok((members, invites)) => reply(StatusCode::OK, json!({ "members": members, "invites": invites, "your_role": role })),
        Err(e) => tenant_error(e.into()),
    }
}

#[derive(Deserialize)]
struct InviteBody {
    email: String,
}

async fn invite(State(ps): S, headers: HeaderMap, Path(slug): Path<String>, Json(b): Json<InviteBody>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.invite(t.id, &s, &b.email).await {
        Ok(id) => {
            let emailed = match &ps.mail {
                Some(m) => match m.invite(b.email.trim(), &t.name, &s.email).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(error = %e, company = %t.slug, "invitation email not sent");
                        false
                    }
                },
                None => false,
            };
            let next = if emailed {
                "We emailed them. They join by signing up with that address and turning on two-factor authentication."
            } else {
                "Ask them to sign up with that email and turn on two-factor authentication; they'll see the company when they sign in."
            };
            reply(StatusCode::CREATED, json!({ "invite_id": id, "emailed": emailed, "next": next }))
        }
        Err(e) => tenant_error(e),
    }
}

async fn revoke_invite(State(ps): S, headers: HeaderMap, Path((slug, id)): Path<(String, Uuid)>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.revoke_invite(t.id, &s, id).await {
        Ok(()) => reply(StatusCode::OK, json!({ "revoked": id })),
        Err(e) => tenant_error(e),
    }
}

async fn remove_member(State(ps): S, headers: HeaderMap, Path((slug, user)): Path<(String, Uuid)>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.remove_member(t.id, &s, user).await {
        Ok(()) => reply(StatusCode::OK, json!({ "removed": user })),
        Err(e) => tenant_error(e),
    }
}

async fn billing(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let (t, _, _) = member!(ps, headers, slug);
    match ps.platform.invoices(t.id).await {
        Ok(invoices) => {
            let list: Vec<Value> = invoices.iter().map(|i| json!({ "invoice": i, "amount_near": i.amount_near() })).collect();
            let p = &ps.platform.plan;
            reply(
                StatusCode::OK,
                json!({
                    "subscription": t.subscription(Utc::now()),
                    "plan": { "name": p.name, "price_usd": p.price_usd, "trial_days": p.trial_days, "network": p.network, "pay_to": p.pay_to },
                    "invoices": list,
                }),
            )
        }
        Err(e) => tenant_error(e.into()),
    }
}

#[derive(Deserialize)]
struct NewInvoice {
    #[serde(default = "one")]
    months: i32,
}

fn one() -> i32 {
    1
}

async fn create_invoice(State(ps): S, headers: HeaderMap, Path(slug): Path<String>, Json(b): Json<NewInvoice>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.create_invoice(t.id, &s, b.months, ps.price.as_ref(), Utc::now()).await {
        Ok(i) => reply(StatusCode::CREATED, json!({ "invoice": i, "amount_near": i.amount_near() })),
        Err(e) => tenant_error(e),
    }
}

#[derive(Deserialize)]
struct VerifyBody {
    tx_hash: String,
    payer: String,
}

async fn verify_invoice(State(ps): S, headers: HeaderMap, Path((slug, id)): Path<(String, Uuid)>, Json(b): Json<VerifyBody>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    match ps.platform.verify_payment(t.id, &s, id, &b.tx_hash, &b.payer, ps.payments.as_ref(), Utc::now()).await {
        Ok(paid_until) => reply(StatusCode::OK, json!({ "status": "paid", "paid_until": paid_until })),
        Err(e) => {
            let status = match e {
                PaymentError::NotFound => StatusCode::NOT_FOUND,
                PaymentError::AlreadyUsed | PaymentError::NotOpen(_) => StatusCode::CONFLICT,
                PaymentError::Other(_) => StatusCode::BAD_GATEWAY,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            reply(status, json!({ "error": e.to_string() }))
        }
    }
}

async fn tenant_dispatch_root(state: S, Path(slug): Path<String>, req: Request) -> Response {
    forward(state, slug, req).await
}

async fn tenant_dispatch(state: S, Path((slug, _rest)): Path<(String, String)>, req: Request) -> Response {
    forward(state, slug, req).await
}

/// The company's own path: everything after `/t/<slug>` exactly as it was
/// sent (still percent-encoded). The access check and the company's router
/// must look at the same string; a decoded copy could hide a `?` or `/`.
fn inner_path(path: &str) -> &str {
    let after_slug = path.strip_prefix("/t/").unwrap_or("");
    match after_slug.find('/') {
        Some(i) => &after_slug[i..],
        None => "/",
    }
}

async fn forward(State(ps): S, slug: String, req: Request) -> Response {
    let t = match ps.platform.by_slug(&slug).await {
        Ok(t) => t,
        Err(e) => return tenant_error(e),
    };
    if t.status == "suspended" {
        return reply(StatusCode::FORBIDDEN, json!({ "error": "this company is suspended; contact the platform operator" }));
    }
    let path = inner_path(req.uri().path()).to_string();
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if MEMBERS_ONLY.contains(&first) {
        let s = match session(&ps, req.headers()).await {
            Ok(s) => s,
            Err(r) => return *r,
        };
        match ps.platform.role(t.id, s.user_id).await {
            Ok(Some(_)) if s.mfa_verified => {}
            Ok(Some(_)) => return reply(StatusCode::FORBIDDEN, json!({ "error": "an MFA-verified session is required" })),
            Ok(None) => return reply(StatusCode::NOT_FOUND, json!({ "error": "company not found" })),
            Err(e) => return tenant_error(e.into()),
        }
    }
    // An expired subscription pauses automation and new job applications;
    // the dashboard stays readable so the company can pay.
    let expired = matches!(t.subscription(Utc::now()), Subscription::Expired { .. });
    if expired && req.method() == axum::http::Method::POST && first == "jobs" {
        return reply(StatusCode::PAYMENT_REQUIRED, json!({ "error": "this company's jobs board is paused" }));
    }
    let router = match ps.router_for(&t).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, tenant = %t.id, "building company router failed");
            return reply(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "this company is unavailable right now" }));
        }
    };
    let (parts, body) = req.into_parts();
    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let Ok(uri) = format!("{path}{query}").parse::<Uri>() else {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "bad path" }));
    };
    // A fresh request: the outer route's path parameters (slug, rest) live in
    // the request extensions, and carrying them over makes every company
    // route with its own path parameter fail to extract it.
    let mut inner = Request::new(body);
    *inner.method_mut() = parts.method;
    *inner.uri_mut() = uri;
    *inner.version_mut() = parts.version;
    *inner.headers_mut() = parts.headers;
    let mut resp = match router.oneshot(inner).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    if expired {
        resp.headers_mut().insert("x-axom-subscription", axum::http::HeaderValue::from_static("expired"));
    }
    resp.map(Body::new)
}

async fn setup_treasury(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    let Some(keys) = ps.treasury.clone() else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "treasury keys are not available on this platform yet (OpenBao provisioner not configured)" }));
    };
    match ps.platform.setup_treasury(t.id, &s, keys.as_ref()).await {
        Ok(account) => {
            // Its dashboard shows the new treasury right away.
            ps.invalidate(t.id).await;
            reply(
            StatusCode::OK,
            json!({ "treasury_account": account, "next": format!("Fund {account} with NEAR ({}); payouts are sent from it.", ps.platform.plan.network) }),
            )
        }
        Err(e) => tenant_error(e),
    }
}

#[derive(Deserialize)]
struct PlanBody {
    goal: String,
    budget_usd: rust_decimal::Decimal,
    deadline: chrono::NaiveDate,
    objectives: Vec<ObjectiveBody>,
}

#[derive(Deserialize)]
struct ObjectiveBody {
    title: String,
    #[serde(default)]
    description: String,
}

/// Set the company's first plan: Combox proposes departments, roles, AI
/// workers, and tasks from the goal and objectives (policy-checked).
async fn set_plan(State(ps): S, headers: HeaderMap, Path(slug): Path<String>, Json(b): Json<PlanBody>) -> Response {
    let (t, s, _) = member!(ps, headers, slug);
    if !s.mfa_verified {
        return reply(StatusCode::FORBIDDEN, json!({ "error": "an MFA-verified session is required" }));
    }
    let Some(planner_cfg) = ps.planner.clone() else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "planning is not configured" }));
    };
    let goal = b.goal.trim();
    let bad = goal.is_empty()
        || goal.chars().count() > 300
        || b.budget_usd <= rust_decimal::Decimal::ZERO
        || b.deadline <= Utc::now().date_naive()
        || b.objectives.is_empty()
        || b.objectives.len() > 20
        || b.objectives.iter().any(|o| o.title.trim().is_empty() || o.title.chars().count() > 200 || o.description.chars().count() > 1000);
    if bad {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "goal (1-300 characters), a budget above 0, a future deadline, and 1-20 objectives (title up to 200, description up to 1000 characters) are required" }),
        );
    }
    let pool = ps.platform.pool(&t).await;
    match sqlx::query_as::<_, (bool,)>("SELECT EXISTS (SELECT 1 FROM axom.plans)").fetch_one(&pool).await {
        Ok((true,)) => return reply(StatusCode::CONFLICT, json!({ "error": "this company already has a plan" })),
        Ok(_) => {}
        Err(e) => return tenant_error(e.into()),
    }
    let plan = axom_core::domain::Plan {
        goal: goal.to_string(),
        budget_usd: b.budget_usd,
        deadline: b.deadline,
        objectives: b
            .objectives
            .iter()
            .map(|o| serde_json::from_value(json!({ "title": o.title.trim(), "description": o.description.trim() })).expect("objective"))
            .collect(),
    };
    let planner = axom_planner::RuleBasedPlanner::new((*planner_cfg).clone());
    match axom_planner::store::bootstrap(&pool, &ps.platform.policy, &planner_cfg, &planner, &plan).await {
        Ok(report) => {
            if let Err(e) = axom_planner::routing::route_queued(&pool, &axom_core::domain::Actor::System).await {
                tracing::error!(error = %e, tenant = %t.id, "routing after plan failed");
            }
            let _ = crate::tenants::audit(&ps.platform.db, Some(t.id), &format!("user:{}", s.user_id), "company.plan", json!({ "departments": report.departments, "workers": report.workers_created, "tasks": report.tasks_created })).await;
            reply(StatusCode::CREATED, json!({ "departments": report.departments, "workers_created": report.workers_created, "tasks_created": report.tasks_created }))
        }
        Err(e) => reply(StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": format!("the plan was not accepted: {e}") })),
    }
}

async fn operator(ps: &PlatformState, headers: &HeaderMap) -> Result<AdminSession, Box<Response>> {
    let s = session(ps, headers).await?;
    if ps.operators.allows(&s) {
        Ok(s)
    } else {
        Err(Box::new(reply(StatusCode::FORBIDDEN, json!({ "error": "platform operators only" }))))
    }
}

async fn operator_companies(State(ps): S, headers: HeaderMap) -> Response {
    if let Err(r) = operator(&ps, &headers).await {
        return *r;
    }
    match ps.platform.operator_overview(Utc::now()).await {
        Ok((companies, totals)) => reply(StatusCode::OK, json!({ "companies": companies, "totals": totals })),
        Err(e) => tenant_error(e.into()),
    }
}

#[derive(Deserialize)]
struct ReasonBody {
    #[serde(default)]
    reason: String,
}

async fn operator_suspend(State(ps): S, headers: HeaderMap, Path(slug): Path<String>, Json(b): Json<ReasonBody>) -> Response {
    let s = match operator(&ps, &headers).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match ps.platform.set_suspended(&slug, true, &s, &b.reason).await {
        Ok(()) => {
            if let Ok(t) = ps.platform.by_slug(&slug).await {
                ps.invalidate(t.id).await;
            }
            reply(StatusCode::OK, json!({ "status": "suspended" }))
        }
        Err(e) => tenant_error(e),
    }
}

async fn operator_resume(State(ps): S, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    let s = match operator(&ps, &headers).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match ps.platform.set_suspended(&slug, false, &s, "").await {
        Ok(()) => reply(StatusCode::OK, json!({ "status": "active" })),
        Err(e) => tenant_error(e),
    }
}

