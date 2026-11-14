//! `AXOM_MULTI_TENANT=true`: the platform. `DATABASE_URL` is the platform
//! database (companies, members, keys, invoices); every company gets its own
//! database on the same server. One API serves all companies under
//! `/t/<company>/`, and a supervisor runs each company's background loop
//! while its trial or subscription is current.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axom_api::auth::AuthConfig;
use axom_core::authz::AuthorizationKey;
use axom_hr::notify::NotifyConfig;
use axom_hr::performance::PerformanceConfig;
use axom_hr::PolicyConfig;
use axom_planner::PlannerConfig;
use axom_platform::billing::{PlanConfig, RpcPayments};
use axom_platform::crypto::SecretBox;
use axom_platform::http::{tenant_url, PlatformState};
use axom_platform::secrets::CompanyConfig;
use axom_platform::treasury::OpenBaoProvisioner;
use axom_platform::Platform;
use axom_tools::price::{HttpPriceFeed, PriceFeed};
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::runtime::Runtime;

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

pub async fn run() -> anyhow::Result<()> {
    let policy = Arc::new(PolicyConfig::load(&PathBuf::from(env_or("AXOM_POLICY_PATH", "config/policy.toml"))).context("loading policy")?);
    let planner = Arc::new(PlannerConfig::load(&PathBuf::from(env_or("AXOM_PLANNER_CONFIG_PATH", "config/planner.toml"))).context("loading planner config")?);
    let perf = Arc::new(PerformanceConfig::load(&PathBuf::from(env_or("AXOM_PERFORMANCE_CONFIG_PATH", "config/performance.toml"))).context("loading performance config")?);
    let secrets = SecretBox::from_env().context("AXOM_SECRETS_KEY must be set to 64 hex characters (generate one with `axom secrets-key`)")?;
    let auth = AuthConfig::from_env().context("SUPABASE_JWKS_URL or SUPABASE_JWT_SECRET must be set: sign-in is required on the platform")?;
    let approval_key = Arc::new(AuthorizationKey::from_env("AXOM_APPROVAL_SIGNING_KEY").map_err(|_| anyhow::anyhow!(crate::signing_key_problem("AXOM_APPROVAL_SIGNING_KEY")))?);
    let attestor = if env_or("AXOM_DISABLE_PAYROLL", "false") == "true" { None } else { AuthorizationKey::from_env("AXOM_ATTESTATION_SIGNING_KEY").ok().map(Arc::new) };
    if attestor.is_none() {
        tracing::warn!("AXOM_ATTESTATION_SIGNING_KEY not set: payroll disabled for all companies");
    }
    let plan = PlanConfig::from_env();
    if plan.pay_to.is_none() {
        tracing::warn!("AXOM_BILLING_ACCOUNT not set: companies can't pay subscriptions yet");
    }
    if plan.network == "mainnet" && std::env::var("AXOM_LIVE").as_deref() != Ok("true") {
        anyhow::bail!("NEAR_NETWORK=mainnet requires AXOM_LIVE=true (after the owner's legal review)");
    }

    // DATABASE_URL holds credentials: never log it.
    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set (the platform database)")?;
    let opts = PgConnectOptions::from_str(&db_url).context("parsing DATABASE_URL")?.options([("search_path", "public")]);
    let db = PgPoolOptions::new().max_connections(10).connect_with(opts).await.context("connecting to the platform database")?;
    let platform = Arc::new(Platform::new(db, secrets, policy.clone(), plan.clone()).await.context("platform migrations")?);
    tracing::info!(price_usd = %plan.price_usd, trial_days = plan.trial_days, network = %plan.network, "multi-tenant platform ready");

    let rpc_url = match plan.network.as_str() {
        "mainnet" => std::env::var("NEAR_RPC_URL").context("NEAR_RPC_URL must be set explicitly for mainnet")?,
        _ => axom_tools::near_rpc::testnet_rpc_url(),
    };
    let price: Arc<dyn PriceFeed> = Arc::new(HttpPriceFeed::from_env());
    let public_api_url = env_or("AXOM_PUBLIC_API_URL", "http://localhost:8080");
    let meet_web_url = env_or("AXOM_MEET_WEB_URL", "http://localhost:5173");
    let dashboard_url = env_or("AXOM_DASHBOARD_URL", "http://localhost:5174");
    let mut state = PlatformState::new(
        platform.clone(),
        auth,
        approval_key,
        price.clone(),
        Arc::new(RpcPayments::new(rpc_url)),
        public_api_url.clone(),
        meet_web_url.clone(),
        dashboard_url.clone(),
    )
    .with_planner(planner)
    .with_agent_callback_url(crate::agent_callback_url());
    // Platform emails (invitations, subscription reminders): SMTP from the
    // environment when configured (dry-run unless AXOM_LIVE=true), else a mock.
    let mailer: Arc<dyn axom_tools::Mailer> = match axom_tools::adapters::smtp::SmtpMailer::from_env() {
        Ok(m) => Arc::new(m),
        // Live: refuse, so an invitation is never reported as emailed and a
        // reminder is never recorded as sent when nothing went out.
        Err(e) if axom_tools::adapters::http::live_from_env() => {
            tracing::warn!(error = %e, "live mode is on but SMTP is not set up: platform emails (invitations, reminders) will fail");
            Arc::new(axom_tools::not_set_up::NotSetUp::new("email (SMTP)", &e))
        }
        Err(_) => {
            tracing::warn!("SMTP_* not set: platform emails (invitations, reminders) are not sent");
            Arc::new(axom_tools::mock::MockTools::new())
        }
    };
    let mail = Arc::new(axom_platform::mail::PlatformMail { mailer, policy: policy.clone(), dashboard_url: dashboard_url.clone() });
    state = state.with_mail(mail.clone());
    let operators = axom_platform::operator::Operators::from_env();
    if operators.is_empty() {
        tracing::warn!("AXOM_PLATFORM_OPERATORS not set: no one can see all companies or suspend them");
    }
    state = state.with_operators(operators);
    match OpenBaoProvisioner::from_env() {
        Some(p) => state = state.with_treasury(Arc::new(p)),
        None => tracing::warn!("OPENBAO_PROVISIONER_TOKEN not set: companies can't create treasuries (payouts) yet"),
    }

    let sup = Supervisor { platform: platform.clone(), mail, policy, perf, attestor, price, network: plan.network.clone(), agent_callback_url: crate::agent_callback_url(), meet_web_url, dashboard_url };
    tokio::spawn(sup.run());

    let bind = env_or("AXOM_API_BIND", "127.0.0.1:8080");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "serving platform API");
    axum::serve(listener, axom_platform::http::router(Arc::new(state))).await?;
    Ok(())
}

struct Supervisor {
    platform: Arc<Platform>,
    mail: Arc<axom_platform::mail::PlatformMail>,
    policy: Arc<PolicyConfig>,
    perf: Arc<PerformanceConfig>,
    attestor: Option<Arc<AuthorizationKey>>,
    price: Arc<dyn PriceFeed>,
    network: String,
    /// Where each company's meeting agent calls back (per-company path added).
    agent_callback_url: String,
    meet_web_url: String,
    dashboard_url: String,
}

/// What a company's runtime was started with; a change restarts it.
#[derive(PartialEq)]
struct Fingerprint {
    keys_updated: Option<DateTime<Utc>>,
    keys: i64,
    owner_email: String,
}

impl Supervisor {
    /// Every minute: start a runtime for each company with a current trial
    /// or subscription, restart it when its keys change, and stop it when
    /// the subscription lapses or the company is suspended.
    async fn run(self) {
        let mut running: HashMap<Uuid, (JoinHandle<()>, Fingerprint)> = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        let mut last_reminders: Option<std::time::Instant> = None;
        loop {
            tick.tick().await;
            let now = Utc::now();
            if last_reminders.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                last_reminders = Some(std::time::Instant::now());
                match self.mail.send_reminders(&self.platform, now).await {
                    Ok(n) if n > 0 => tracing::info!(sent = n, "subscription reminders"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "subscription reminders failed"),
                }
            }
            let tenants = match self.platform.active_tenants().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!(error = %e, "listing companies failed");
                    continue;
                }
            };
            let mut wanted = std::collections::HashSet::new();
            for t in tenants {
                if !t.subscription(now).is_usable() {
                    continue;
                }
                wanted.insert(t.id);
                let fp: Result<(Option<DateTime<Utc>>, i64), _> = sqlx::query_as("SELECT max(updated_at), count(*) FROM platform.secrets WHERE tenant_id = $1").bind(t.id).fetch_one(&self.platform.db).await;
                let Ok((keys_updated, keys)) = fp else { continue };
                let fp = Fingerprint { keys_updated, keys, owner_email: t.owner_email.clone() };
                let alive = running.get(&t.id).is_some_and(|(h, old)| !h.is_finished() && *old == fp);
                if alive {
                    continue;
                }
                if let Some((h, _)) = running.remove(&t.id) {
                    h.abort();
                }
                let config = match self.platform.config(t.id).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(error = %e, company = %t.slug, "loading company keys failed");
                        continue;
                    }
                };
                let rt = Runtime {
                    label: t.slug.clone(),
                    pool: self.platform.pool(&t).await,
                    policy: self.policy.clone(),
                    perf: self.perf.clone(),
                    notify: NotifyConfig { owner_email: t.owner_email.clone(), dashboard_url: format!("{}/c/{}", self.dashboard_url.trim_end_matches('/'), t.slug) },
                    config: Arc::new(CompanyConfig(config)),
                    attestor: self.attestor.clone(),
                    network: self.network.clone(),
                    price: self.price.clone(),
                    api_url: tenant_url(&self.agent_callback_url, &t.slug),
                    meet_web_url: tenant_url(&self.meet_web_url, &t.slug),
                    secrets: Some(self.platform.secrets.clone()),
                    tool_guard: axom_tools::adapters::mcp::UrlGuard::strict(),
                };
                running.insert(t.id, (tokio::spawn(rt.run()), fp));
            }
            running.retain(|id, (h, _)| {
                if wanted.contains(id) {
                    true
                } else {
                    tracing::info!(company = %id, "stopping company runtime (subscription lapsed or company suspended)");
                    h.abort();
                    false
                }
            });
        }
    }
}

const IMPORT_USAGE: &str = "usage: axom import-company --database <existing db> --slug <address> --name <company name> --owner-email <email> --owner-user-id <supabase user id>
  DATABASE_URL must point at the platform database; the existing database must be on the same server.";

/// `axom import-company ...`: register an existing single-company database
/// as a company on the platform (see `Platform::import_tenant`).
pub async fn import_company(args: Vec<String>) -> anyhow::Result<()> {
    let mut flags = HashMap::new();
    let mut it = args.into_iter();
    while let Some(k) = it.next() {
        let Some(name) = k.strip_prefix("--") else { anyhow::bail!("unexpected argument {k}\n{IMPORT_USAGE}") };
        let v = it.next().ok_or_else(|| anyhow::anyhow!("--{name} needs a value\n{IMPORT_USAGE}"))?;
        flags.insert(name.to_string(), v);
    }
    let get = |k: &str| flags.get(k).cloned().ok_or_else(|| anyhow::anyhow!("--{k} is required\n{IMPORT_USAGE}"));
    let (database, slug, name, owner_email) = (get("database")?, get("slug")?, get("name")?, get("owner-email")?);
    let owner_user_id: Uuid = get("owner-user-id")?.parse().context("--owner-user-id must be a UUID (the user's id in Supabase Auth)")?;

    let policy = Arc::new(PolicyConfig::load(&PathBuf::from(env_or("AXOM_POLICY_PATH", "config/policy.toml"))).context("loading policy")?);
    let secrets = SecretBox::from_env().context("AXOM_SECRETS_KEY must be set (the platform's)")?;
    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set (the platform database)")?;
    let opts = PgConnectOptions::from_str(&db_url).context("parsing DATABASE_URL")?.options([("search_path", "public")]);
    let db = PgPoolOptions::new().max_connections(3).connect_with(opts).await.context("connecting to the platform database")?;
    let platform = Platform::new(db, secrets, policy, PlanConfig::from_env()).await.context("platform migrations")?;
    let t = platform
        .import_tenant(&axom_platform::tenants::Import { database: &database, slug: &slug, name: &name, owner_user_id, owner_email: &owner_email }, Utc::now())
        .await?;
    println!("Imported {database} as \"{}\" at /t/{}. Free trial until {}.", t.name, t.slug, t.trial_ends_at.format("%Y-%m-%d"));
    println!("The owner signs in to the dashboard with {owner_email}. Its jobs board and API are now under /t/{}/.", t.slug);
    println!("If it had a NEAR treasury, set it up again from Settings (the platform uses a key per company).");
    Ok(())
}

