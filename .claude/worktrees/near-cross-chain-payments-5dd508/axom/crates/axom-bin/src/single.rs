//! Single-company mode: integration keys can be set from the dashboard
//! (stored encrypted in the company database) and override `.env`. When an
//! admin saves or removes one, the API's adapters are rebuilt on the next
//! request and the background loop restarts within a minute.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axom_api::{ApprovalConfig, AppState, Services};
use axom_core::secretbox::SecretBox;
use axom_hr::notify::NotifyConfig;
use axom_hr::PolicyConfig;
use axom_tools::config::{ConfigSource, MapSource, WithEnv};
use axom_tools::integrations::INTERVIEW_KEYS;
use axom_tools::mock::MockTools;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::Router;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tower::ServiceExt;

use crate::runtime::Runtime;

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

/// The dashboard's saved settings (empty if they can't be read).
async fn own_settings(pool: &PgPool, secrets: Option<&SecretBox>) -> MapSource {
    axom_api::integrations::load(pool, secrets).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "loading saved integration settings failed; using .env only");
        MapSource::default()
    })
}

/// Meeting and summary services: real adapters when configured (dry-run
/// unless AXOM_LIVE=true), mocks otherwise.
pub fn build_services(policy: &PolicyConfig, own: &MapSource) -> Services {
    use axom_tools::adapters::{livekit::LiveKit, smtp::SmtpMailer, opendesk::OpendeskCrm};
    let cfg = WithEnv(own.clone());
    let mock = Arc::new(MockTools::new());
    // In live mode a service that can't be built refuses every call; a mock
    // there would report invites and emails as sent.
    let live = axom_tools::adapters::http::live_from_env();
    let not_set_up = |service: &'static str, e: &anyhow::Error| {
        tracing::warn!(service, error = %e, "live mode is on but this service is not set up; its actions will fail until it is");
        Arc::new(axom_tools::not_set_up::NotSetUp::new(service, e))
    };
    let rooms: Arc<dyn axom_tools::MeetingRooms> = match LiveKit::from_env() {
        Ok(lk) => Arc::new(lk),
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
            // Set but unusable (a wrong CRM_URL): say so instead of quietly
            // filing summaries nowhere.
            if cfg.get("CRM_API_KEY").is_some() || cfg.get("CRM_URL").is_some() {
                tracing::warn!(error = %e, "CRM (opendesk) settings can't be used; meeting summaries won't reach the CRM");
            }
            mock.clone()
        }
    };
    Services {
        policy: Arc::new(policy.clone()),
        notify: NotifyConfig::from_env(),
        rooms,
        mailer,
        crm,
        livekit_ws_url: std::env::var("LIVEKIT_URL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "ws://localhost:7880".into()),
        public_api_url: crate::agent_callback_url(),
        meet_web_url: env_or("AXOM_MEET_WEB_URL", "http://localhost:5173"),
        // Only keys saved in the dashboard; otherwise the agent uses its own.
        agent_keys: INTERVIEW_KEYS.iter().filter_map(|k| own.get(k).map(|v| (k.to_string(), v))).collect(),
    }
}

/// The API, rebuilt after a setting changes.
pub struct LiveApi {
    pub pool: PgPool,
    pub approvals: Option<ApprovalConfig>,
    pub policy: Arc<PolicyConfig>,
    pub secrets: Option<SecretBox>,
    pub generation: Arc<AtomicU64>,
    current: RwLock<Option<(u64, Router)>>,
}

impl LiveApi {
    pub fn new(pool: PgPool, approvals: Option<ApprovalConfig>, policy: Arc<PolicyConfig>, secrets: Option<SecretBox>) -> Self {
        Self { pool, approvals, policy, secrets, generation: Arc::default(), current: RwLock::default() }
    }

    async fn router(&self) -> Router {
        let gen = self.generation.load(Ordering::SeqCst);
        if let Some((g, r)) = self.current.read().await.as_ref() {
            if *g == gen {
                return r.clone();
            }
        }
        let own = own_settings(&self.pool, self.secrets.as_ref()).await;
        let cfg = WithEnv(own.clone());
        let state = AppState {
            approvals: self.approvals.clone(),
            services: Some(Arc::new(build_services(&self.policy, &own))),
            mcp_token: cfg.get("AXOM_MCP_TOKEN").filter(|t| t.len() >= 32),
            secrets: self.secrets.clone(),
            local_integrations: true,
            settings_generation: self.generation.clone(),
            owner_email: std::env::var("AXOM_OWNER_EMAIL").ok().filter(|e| !e.trim().is_empty()),
            ..AppState::new(self.pool.clone())
        };
        let r = axom_api::routes(state);
        *self.current.write().await = Some((gen, r.clone()));
        r
    }

    /// The whole API as one router (with CORS).
    pub fn into_router(self) -> Router {
        let live = Arc::new(self);
        Router::new()
            .fallback(move |req: Request| {
                let live = live.clone();
                async move {
                    let r: Response = live.router().await.oneshot(req).await.into_response();
                    r
                }
            })
            .layer(axom_api::cors())
    }
}

/// Runs the company's background loop with the current settings, and
/// restarts it when a setting is saved or removed (checked every 30 seconds).
pub async fn supervise(pool: PgPool, secrets: Option<SecretBox>, make: impl Fn(Arc<dyn ConfigSource>) -> Runtime) {
    type Fingerprint = (Option<chrono::DateTime<chrono::Utc>>, i64);
    let mut running: Option<(tokio::task::JoinHandle<()>, Fingerprint)> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let fp = match axom_api::integrations::fingerprint(&pool).await {
            Ok(fp) => fp,
            Err(e) => {
                tracing::error!(error = %e, "checking integration settings failed");
                continue;
            }
        };
        if running.as_ref().is_some_and(|(h, old)| !h.is_finished() && *old == fp) {
            continue;
        }
        if let Some((h, _)) = running.take() {
            tracing::info!("integration settings changed: restarting the background loop");
            h.abort();
        }
        let own = own_settings(&pool, secrets.as_ref()).await;
        let rt = make(Arc::new(WithEnv(own)));
        running = Some((tokio::spawn(rt.run()), fp));
    }
}
