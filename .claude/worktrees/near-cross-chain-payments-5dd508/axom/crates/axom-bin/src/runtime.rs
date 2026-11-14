//! The background loop for one company: hiring, tax-form sync, retention,
//! approvals expiry, interview expiry and reminders, payroll, performance
//! reviews, and owner notifications.
//!
//! A single-company deployment runs one of these against `DATABASE_URL`
//! with adapters from the environment. The multi-tenant platform runs one
//! per company with a paid-up (or trial) subscription, against that
//! company's database and with adapters built from its own stored keys.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axom_core::authz::AuthorizationKey;
use axom_core::domain::Actor;
use axom_hr::notify::{self, NotifyConfig};
use axom_hr::payroll::{self, PayrollRun};
use axom_hr::performance::PerformanceConfig;
use axom_hr::{payouts, reviews, PolicyConfig};
use axom_planner::{routing, runner};
use axom_tools::config::ConfigSource;
use axom_tools::mock::MockTools;
use axom_tools::price::PriceFeed;
use sqlx::PgPool;

pub struct Runtime {
    /// For logs: "default" or the company's address.
    pub label: String,
    pub pool: PgPool,
    pub policy: Arc<PolicyConfig>,
    pub perf: Arc<PerformanceConfig>,
    pub notify: NotifyConfig,
    /// Where adapters read their settings (environment or company keys).
    pub config: Arc<dyn ConfigSource>,
    /// Payroll needs the attestation key; without it payroll doesn't run.
    pub attestor: Option<Arc<AuthorizationKey>>,
    pub network: String,
    pub price: Arc<dyn PriceFeed>,
    /// Public base URLs for links in messages (company-specific on the platform).
    pub api_url: String,
    pub meet_web_url: String,
    /// Decrypts admin-defined tools' tokens (`AXOM_SECRETS_KEY`).
    pub secrets: Option<axom_core::secretbox::SecretBox>,
    /// Where admin-defined tools may connect.
    pub tool_guard: axom_tools::adapters::mcp::UrlGuard,
}

impl Runtime {
    /// Loop every 15 seconds. Tasks are picked up as soon as they are due: a
    /// new or requeued task wakes the loop at once (Postgres `NOTIFY` on
    /// `axom_tasks`), a scheduled task wakes it at its `start_at`, and every
    /// minute catches anything missed. Hiring, tax sync, and retention run
    /// hourly; reviews, approvals expiry, and notifications each tick.
    pub async fn run(self) {
        let label = self.label.clone();
        let pool = &self.pool;
        let cfg = self.config.as_ref();
        let tools = axom_hr::custom_tools::CustomToolExecutor {
            pool: pool.clone(),
            secrets: self.secrets.clone(),
            guard: self.tool_guard,
            live: axom_tools::adapters::http::live_from_env(),
            // Standard tool names run only once a server is connected under them.
            fallback: &axom_tools::Unconnected,
        };
        let intake = axom_api::intake::DbTaskIntake { pool: pool.clone() };
        let mut last_tasks: Option<Instant> = None;

        // Real adapters when configured (dry-run unless AXOM_LIVE=true). When
        // one can't be built, the mock stands in outside live mode; in live
        // mode a mock would report mail as sent and contracts as signed, so
        // the service refuses every call instead and says what is missing.
        let live = axom_tools::adapters::http::live_from_env();
        let not_set_up = |service: &'static str, e: &anyhow::Error| {
            tracing::warn!(company = %label, service, error = %e, "live mode is on but this service is not set up; its actions will fail until it is");
            axom_tools::not_set_up::NotSetUp::new(service, e)
        };
        let mailer: Arc<dyn axom_tools::Mailer> = match axom_tools::adapters::smtp::SmtpMailer::from_config(cfg) {
            Ok(m) => Arc::new(m),
            Err(e) if live => Arc::new(not_set_up("email (SMTP)", &e)),
            Err(_) => Arc::new(MockTools::new()),
        };
        // ForeMan's own jobs board by default (hires paid in NEAR); Upwork only
        // when chosen, and those hires are paid through Upwork, never in NEAR.
        let market: Box<dyn axom_tools::FreelanceMarketplace> = if cfg.get("AXOM_MARKETPLACE").as_deref() == Some("upwork") {
            match axom_tools::adapters::upwork::UpworkMcp::from_config(cfg) {
                Ok(u) => Box::new(u),
                Err(e) if live => Box::new(not_set_up("Upwork", &e)),
                Err(_) => Box::new(MockTools::new()),
            }
        } else {
            Box::new(axom_hr::board::AxomBoard::new(pool.clone(), mailer.clone()))
        };
        let rooms: Box<dyn axom_tools::MeetingRooms> = match axom_tools::adapters::livekit::LiveKit::from_env() {
            Ok(l) => Box::new(l),
            Err(e) if live => Box::new(not_set_up("video meetings (LiveKit)", &e)),
            Err(_) => Box::new(MockTools::new()),
        };
        let contracts: Box<dyn axom_tools::ContractService> = match axom_tools::adapters::docuseal::DocuSeal::from_config(cfg) {
            Ok(d) => Box::new(d),
            Err(e) if live => Box::new(not_set_up("contracts (DocuSeal)", &e)),
            Err(_) => Box::new(MockTools::new()),
        };
        // AI workers think with Claude when an Anthropic key is set.
        let brain = axom_tools::llm::Anthropic::from_config(cfg);
        tracing::info!(company = %label, marketplace = market.name(), ai_workers = if brain.is_some() { "thinking (Claude)" } else { "tools only (no Anthropic key)" }, "company runtime started");

        // Organizer on interview calendar invites.
        let organizer = cfg.get("SMTP_FROM").unwrap_or_else(|| "foreman@localhost".into());
        let mut last_payroll: Option<Instant> = None;
        let mut last_hiring: Option<Instant> = None;
        let mut last_digest = chrono::Utc::now();
        let brain = brain.as_ref().map(|b| b as &dyn axom_tools::llm::Llm);
        // New and requeued tasks wake the loop right away. Without the
        // listener, tasks are still picked up on the minute.
        let mut listener = match task_listener(pool).await {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!(company = %label, error = %e, "cannot listen for new tasks; picking them up every minute instead");
                None
            }
        };
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            // A scheduled task wakes the loop at its start time.
            let next_start = axom_core::db::next_task_start(pool).await.unwrap_or_else(|e| {
                tracing::error!(company = %label, error = %e, "reading the next task start failed");
                None
            });
            let woken = tokio::select! {
                _ = tick.tick() => false,
                _ = task_ready(&mut listener, &label) => true,
                _ = sleep_until(next_start) => true,
            };
            // The platform's policy with this company's stricter rules (lower
            // approval threshold, shorter retention), re-read every pass.
            let effective = axom_hr::company_policy::effective(&self.policy, pool).await;
            let policy = &effective;
            if woken {
                // Only the task pass: the rest keeps its own schedule.
                last_tasks = Some(Instant::now());
                task_pass(&label, pool, policy, market.as_ref(), &tools, &intake, brain).await;
                continue;
            }
            let now = chrono::Utc::now();
            if last_tasks.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                last_tasks = Some(Instant::now());
                task_pass(&label, pool, policy, market.as_ref(), &tools, &intake, brain).await;
            }
            if last_hiring.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                last_hiring = Some(Instant::now());
                match axom_hr::hiring::process_open_requests(pool, policy, market.as_ref()).await {
                    Ok(done) if !done.is_empty() => {
                        tracing::info!(company = %label, ?done, "hiring requests processed");
                        // New hires start on their tasks now, not on the next minute.
                        task_pass(&label, pool, policy, market.as_ref(), &tools, &intake, brain).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(company = %label, error = %e, "hiring failed"),
                }
                let hsvc = axom_hr::hiring::HiringServices {
                    market: market.as_ref(),
                    rooms: rooms.as_ref(),
                    contracts: contracts.as_ref(),
                    notify: &self.notify,
                    api_url: &self.api_url,
                    meet_web_url: &self.meet_web_url,
                };
                match axom_hr::gates::sync_missing_tax_forms(pool, contracts.as_ref()).await {
                    Ok(n) if n > 0 => tracing::info!(company = %label, recorded = n, "tax forms synced from DocuSeal"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(company = %label, error = %e, "tax form sync failed"),
                }
                match axom_hr::privacy::purge_expired_candidates(pool, policy, now).await {
                    Ok(n) if n > 0 => tracing::info!(company = %label, redacted = n, "candidate data past retention redacted"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(company = %label, error = %e, "retention purge failed"),
                }
                match axom_hr::hiring::advance_hiring(pool, policy, &hsvc, now).await {
                    Ok(log) if !log.is_empty() => tracing::info!(company = %label, ?log, "hiring pipeline advanced"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(company = %label, error = %e, "hiring pipeline failed"),
                }
                if let Err(e) = axom_hr::board::close_finished_jobs(pool).await {
                    tracing::error!(company = %label, error = %e, "closing filled board jobs failed");
                }
            }
            if now - last_digest >= chrono::Duration::hours(24) {
                if let Err(e) = axom_hr::hiring::daily_digest(pool, last_digest).await {
                    tracing::error!(company = %label, error = %e, "daily digest failed");
                }
                last_digest = now;
            }
            // Timed interviews: expire missed meetings, remind invitees.
            match axom_hr::meetings::expire_meetings(pool, policy, rooms.as_ref(), now).await {
                Ok(ids) if !ids.is_empty() => tracing::info!(company = %label, ?ids, "interviews expired"),
                Ok(_) => {}
                Err(e) => tracing::error!(company = %label, error = %e, "expiring interviews failed"),
            }
            match axom_hr::meetings::send_due_reminders(pool, policy, mailer.as_ref(), market.as_ref(), &self.meet_web_url, &organizer, now).await {
                Ok(sent) if !sent.is_empty() => tracing::info!(company = %label, reminders = sent.len(), "interview reminders sent"),
                Ok(_) => {}
                Err(e) => tracing::error!(company = %label, error = %e, "interview reminders failed"),
            }
            if let Err(e) = payouts::expire_pending(pool, policy, now).await {
                tracing::error!(company = %label, error = %e, "expiring approvals failed");
            }
            if let Some(key) = &self.attestor {
                if last_payroll.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                    last_payroll = Some(Instant::now());
                    // First keep what is already approved sendable (or give
                    // it up cleanly), so nothing sits approved forever.
                    match payroll::refresh_authorizations(pool, policy, key, chrono::Utc::now()).await {
                        Ok(r) if r != payroll::Refreshed::default() => {
                            tracing::info!(company = %label, issued = r.issued.len(), renewed = r.renewed.len(), abandoned = r.abandoned.len(), "payout authorizations refreshed")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::error!(company = %label, error = %e, "refreshing payout authorizations failed"),
                    }
                    match self.price.quote().await {
                        Ok(q) => {
                            // Payroll's clock starts once the price is in
                            // hand. The tick's `now` was taken before the
                            // quote was fetched, which makes the quote look
                            // like it is from the future; the policy treats
                            // that as stale and sends every payout, however
                            // small, to manual approval.
                            let now = payroll::clock_after_quote(&q, chrono::Utc::now());
                            // Every week with unpaid work, not only the last one.
                            let periods = match payroll::periods_to_run(pool, now).await {
                                Ok(p) => p,
                                Err(e) => {
                                    tracing::error!(company = %label, error = %e, "listing pay periods failed; running the last week only");
                                    vec![payroll::last_complete_week(now)]
                                }
                            };
                            for period in periods {
                                let run = PayrollRun { policy, price: Some(q), fee_near: rust_decimal::Decimal::new(1, 3), network: &self.network, attestor: key, period };
                                match payroll::run_payroll(pool, &run, now).await {
                                    Ok(lines) => tracing::info!(company = %label, period_start = %period.0, lines = lines.len(), "payroll run"),
                                    Err(e) => tracing::error!(company = %label, period_start = %period.0, error = %e, "payroll failed"),
                                }
                            }
                        }
                        Err(e) => tracing::warn!(company = %label, error = %e, "no NEAR price; payroll skipped this hour"),
                    }
                }
            }
            match reviews::run_due_reviews(pool, policy, &self.perf, &self.notify, chrono::Utc::now()).await {
                Ok(out) => {
                    for o in out.iter().filter(|o| o.fired.is_some()) {
                        tracing::warn!(company = %label, worker_id = %o.review.worker_id, "worker fired");
                    }
                }
                Err(e) => tracing::error!(company = %label, error = %e, "review run failed"),
            }
            if !self.notify.owner_email.is_empty() {
                match notify::deliver_pending(pool, policy, &self.notify, mailer.as_ref()).await {
                    Ok(r) if !r.sent.is_empty() || !r.failed.is_empty() => {
                        tracing::info!(company = %label, sent = r.sent.len(), failed = r.failed.len(), "owner notifications")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(company = %label, error = %e, "notification delivery failed"),
                }
            }
        }
    }
}

/// Pick up work: route every due task, hire at once for any that no worker
/// covers (an AI agent straight away, or a freelancer job posting), route
/// again so a new AI hire gets its task, then run everything assigned.
async fn task_pass(
    label: &str,
    pool: &PgPool,
    policy: &axom_hr::PolicyConfig,
    market: &dyn axom_tools::FreelanceMarketplace,
    tools: &dyn axom_tools::ToolExecutor,
    intake: &dyn axom_tools::TaskIntake,
    brain: Option<&dyn axom_tools::llm::Llm>,
) {
    let route = || async {
        match routing::route_queued(pool, &Actor::System).await {
            Ok(r) => {
                if !r.assigned.is_empty() || !r.hiring_requests.is_empty() {
                    tracing::info!(company = %label, assigned = r.assigned.len(), hiring_requests = r.hiring_requests.len(), "routed tasks");
                }
                r.hiring_requests.len()
            }
            Err(e) => {
                tracing::error!(company = %label, error = %e, "routing failed");
                0
            }
        }
    };
    if route().await > 0 {
        // Don't leave new requests for the hourly pass.
        match axom_hr::hiring::process_open_requests(pool, policy, market).await {
            Ok(done) if !done.is_empty() => {
                tracing::info!(company = %label, ?done, "hiring requests processed");
                route().await;
            }
            Ok(_) => {}
            Err(e) => tracing::error!(company = %label, error = %e, "hiring failed"),
        }
    }
    match runner::run_assigned_with(pool, policy, tools, intake, brain).await {
        Ok(r) if !(r.done.is_empty() && r.sent_to_intake.is_empty() && r.blocked.is_empty() && r.failed.is_empty()) => {
            tracing::info!(company = %label, done = r.done.len(), intake = r.sent_to_intake.len(), blocked = r.blocked.len(), failed = r.failed.len(), "tasks run")
        }
        Ok(_) => {}
        Err(e) => tracing::error!(company = %label, error = %e, "task run failed"),
    }
}

/// Channel the `tasks_notify_ready` trigger posts to when a task is queued.
const TASKS_CHANNEL: &str = "axom_tasks";

async fn task_listener(pool: &PgPool) -> Result<sqlx::postgres::PgListener, sqlx::Error> {
    let mut l = sqlx::postgres::PgListener::connect_with(pool).await?;
    l.listen(TASKS_CHANNEL).await?;
    Ok(l)
}

/// Resolves when a task has been queued. Several at once count as one
/// wake-up. Never resolves without a listener.
async fn task_ready(listener: &mut Option<sqlx::postgres::PgListener>, label: &str) {
    let Some(l) = listener else { return std::future::pending().await };
    match l.recv().await {
        Ok(_) => while l.next_buffered().is_some() {},
        Err(e) => {
            // The listener reconnects on the next call; don't spin meanwhile.
            tracing::warn!(company = %label, error = %e, "task listener lost its connection");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

/// Resolves at `at` (at once if it has passed); never without a time.
async fn sleep_until(at: Option<chrono::DateTime<chrono::Utc>>) {
    match at {
        Some(at) => tokio::time::sleep((at - chrono::Utc::now()).to_std().unwrap_or_default()).await,
        None => std::future::pending().await,
    }
}
