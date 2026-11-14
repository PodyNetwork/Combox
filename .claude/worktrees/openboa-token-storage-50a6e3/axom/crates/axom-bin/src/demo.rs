//! Seed data and the judge-mode scenario, shared by `axom demo` (real NEAR
//! testnet payout through the signer service) and the end-to-end test
//! (mocks only).
//!
//! The scenario: boot from the "launch a SaaS product in 90 days" plan, route
//! tasks, run one AI interview with disclosure and consent, fire a seeded low
//! performer, and run payroll so that one payout under USD 50 is sent, one of
//! USD 50 or more waits for approval, and one that would push the month past
//! the cap is blocked. A fourth freelancer is paid in USDC on Base through
//! NEAR Intents (cross-chain): the NEAR leg is a real testnet transaction.

use std::path::Path;

use anyhow::Context as _;
use axom_core::db;
use axom_core::domain::{Actor, PayUnit, Plan, Worker, WorkerKind};
use axom_core::near::PublicKey;
use axom_core::state::{PayoutStatus, WorkerStatus};
use axom_core::Uuid;
use axom_hr::firing::FireReport;
use axom_core::intents::Destination;
use axom_hr::{destinations, gates};
use axom_hr::meetings;
use axom_hr::notify::NotifyConfig;
use axom_hr::payouts::{self, PayoutRequest};
use axom_hr::payroll::{self, PayrollOutcome, PayrollRun};
use axom_hr::performance::PerformanceConfig;
use axom_hr::{cap, reviews, routing, PolicyConfig, PriceQuote};
use axom_planner::{runner, store, PlannerConfig, RuleBasedPlanner};
use axom_tools::{MeetingRooms, TaxFormKind, TaskIntake, ToolExecutor};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

pub struct Configs {
    pub policy: PolicyConfig,
    pub planner: PlannerConfig,
    pub perf: PerformanceConfig,
    pub plan: Plan,
}

impl Configs {
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let read = |p: &str| std::fs::read_to_string(root.join(p)).with_context(|| format!("reading {p}"));
        Ok(Self {
            policy: PolicyConfig::parse(&read("config/policy.toml")?)?,
            planner: PlannerConfig::parse(&read("config/planner.toml")?)?,
            perf: PerformanceConfig::parse(&read("config/performance.toml")?)?,
            plan: serde_json::from_str(&read("config/master_plan.json")?)?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedReport {
    pub plan_id: Uuid,
    pub plan_reused: bool,
    pub tasks_assigned: usize,
    pub hiring_requests: usize,
    pub tasks_done_by_mock_tools: usize,
    pub low_performer: Uuid,
    pub historical_payouts_usd: Decimal,
}

/// Boot from the master plan, route, run mock tools, seed a low performer
/// with 20 days of poor task history, and seed this month's earlier payouts
/// so the cap meter shows partial usage. Idempotent for the plan (boot reuses
/// it); call once per fresh database for the rest.
pub async fn seed(pool: &PgPool, c: &Configs, tools: &dyn ToolExecutor, intake: &dyn TaskIntake, now: DateTime<Utc>) -> anyhow::Result<SeedReport> {
    cap::seed_cap(pool, &c.policy).await?;
    let planner = RuleBasedPlanner::new(c.planner.clone());
    let boot = store::bootstrap(pool, &c.policy, &c.planner, &planner, &c.plan).await?;
    let routed = routing::route_queued(pool, &Actor::System).await?;
    let run = runner::run_assigned(pool, &c.policy, tools, intake).await?;

    // The low performer: an AI agent with 20 days of late and failed work.
    let low = Worker {
        id: Uuid::new_v4(),
        name: "Ops Agent (seeded low performer)".into(),
        kind: WorkerKind::AiAgent,
        role: "Operations Agent".into(),
        status: WorkerStatus::Active,
        skills: ["operations".to_string()].into(),
        tool_allowlist: ["docuseal".to_string(), "smtp".to_string()].into(),
        pay_rate_usd: Decimal::ONE,
        pay_unit: PayUnit::Task,
        wallet_address: None,
        kpis: vec![],
    };
    db::insert_worker(pool, None, None, &low).await?;
    let start = now - Duration::days(20);
    sqlx::query("UPDATE axom.workers SET created_at = $2, model = 'seed-worker-model' WHERE id = $1").bind(low.id).bind(start).execute(pool).await?;
    for i in 0..24 {
        let at = start + Duration::hours(10 + i * 19); // spread over 20 days
        let done = i % 2 == 0; // 50% completion
        let deadline = (at - Duration::days(1)).date_naive(); // every completed task late
        sqlx::query(
            "INSERT INTO axom.tasks (id, objective, required_tools, required_skills, deadline, assignee, status, cost_usd, finished_at, completed_at, created_at)
             VALUES ($1, 'Seeded ops task', '{docuseal}', '{operations}', $2, $3, $4, 0.40, $5, CASE WHEN $6 THEN $5 END, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(deadline)
        .bind(low.id)
        .bind(if done { "done" } else { "failed" })
        .bind(at)
        .bind(done)
        .execute(pool)
        .await?;
    }
    // An open task the low performer holds (reassigned when fired).
    sqlx::query("INSERT INTO axom.tasks (id, objective, required_tools, required_skills, assignee, status) VALUES ($1, 'Prepare contractor paperwork', '{docuseal}', '{operations}', $2, 'assigned')")
        .bind(Uuid::new_v4())
        .bind(low.id)
        .execute(pool)
        .await?;

    // Earlier payouts this month (history only: no on-chain transaction).
    let quote = PriceQuote { usd_per_near: Decimal::new(3, 0), fetched_at: now };
    let mut historical = Decimal::ZERO;
    for (i, amount) in [45, 45, 45, 45].into_iter().enumerate() {
        let req = PayoutRequest { recipient: format!("seed-history-{i}.testnet"), amount_usd: Decimal::from(amount), ..Default::default() };
        let created = payouts::create_payout(pool, &c.policy, &Actor::System, &req, Some(quote), now).await?;
        payouts::mark_sent(pool, &c.policy, created.payout_id, now).await?;
        payouts::move_payout(pool, &Actor::System, created.payout_id, PayoutStatus::Finalized, "seeded history (no on-chain transaction)", c.policy.version(), now).await?;
        historical += Decimal::from(amount);
    }
    Ok(SeedReport {
        plan_id: boot.plan_id,
        plan_reused: boot.reused,
        tasks_assigned: routed.assigned.len(),
        hiring_requests: routed.hiring_requests.len(),
        tasks_done_by_mock_tools: run.done.len(),
        low_performer: low.id,
        historical_payouts_usd: historical,
    })
}

/// Create the interview meeting, assign the interviewer, and join as the
/// candidate (which sends it in). Returns the meeting id and the agent's job
/// metadata (what LiveKit would deliver).
pub async fn interview(pool: &PgPool, c: &Configs, rooms: &dyn MeetingRooms, api_url: &str, metadata_of: impl Fn() -> Option<String>) -> anyhow::Result<(Uuid, Value)> {
    let m = meetings::create_meeting(pool, &c.policy, rooms, &Actor::agent("hr"), "Interview: Brand Designer (judge demo)", Utc::now(), &["candidate@example.com".into()], None).await?;
    let questions = vec![
        "Tell me about a recent logo or brand project.".to_string(),
        "How do you make sure work is delivered on time?".to_string(),
    ];
    meetings::dispatch_agent(pool, &c.policy, rooms, &Actor::agent("hr"), m.meeting_id, "Structured interview for a freelance brand designer.", &questions, None, api_url).await?;
    meetings::join(pool, &c.policy, rooms, "ws://localhost:7880 (mock)", api_url, &m.invites[0].1).await?;
    let metadata: Value = serde_json::from_str(&metadata_of().context("no dispatch metadata recorded")?)?;
    Ok((m.meeting_id, metadata))
}

/// Run the two review periods that fire the seeded low performer.
pub async fn fire_low_performer(pool: &PgPool, c: &Configs, notify: &NotifyConfig, low: Uuid, now: DateTime<Utc>) -> anyhow::Result<FireReport> {
    reviews::run_due_reviews(pool, &c.policy, &c.perf, notify, now - Duration::days(13)).await?;
    let out = reviews::run_due_reviews(pool, &c.policy, &c.perf, notify, now).await?;
    out.into_iter()
        .find(|o| o.review.worker_id == low)
        .and_then(|o| o.fired)
        .context("the seeded low performer was not fired")
}

#[derive(Debug, Clone, Serialize)]
pub struct PayoutDemo {
    pub small: Uuid,
    pub small_recipient: String,
    pub held: Uuid,
    pub blocked: Uuid,
    /// Paid in USDC on Base through NEAR Intents.
    pub cross_chain: Uuid,
    pub cross_chain_destination: Destination,
}

/// USDC on Base, as a NEAR Intents (1Click) asset id.
pub const USDC_BASE_ASSET: &str = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
/// A placeholder Base address for the demo worker (testnet: the solver side is simulated).
pub const DEMO_BASE_ADDRESS: &str = "0x000000000000000000000000000000000000A11E";

/// A freelancer with a verified address (the demo plays the worker and signs
/// the challenge with the worker's own key), a tax form on file, and
/// `tasks` completed tasks at `rate` in the pay period.
async fn ready_freelancer(pool: &PgPool, name: &str, rate: Decimal, tasks: i64, period: (DateTime<Utc>, DateTime<Utc>), network: &str) -> anyhow::Result<(Uuid, String)> {
    let (w, account, _) = ready_freelancer_keyed(pool, name, rate, tasks, period, network).await?;
    Ok((w, account))
}

async fn ready_freelancer_keyed(pool: &PgPool, name: &str, rate: Decimal, tasks: i64, period: (DateTime<Utc>, DateTime<Utc>), network: &str) -> anyhow::Result<(Uuid, String, SigningKey)> {
    let w = Worker {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: WorkerKind::HumanFreelancer,
        role: "Brand Designer".into(),
        status: WorkerStatus::Active,
        skills: ["visual_design".to_string()].into(),
        tool_allowlist: Default::default(),
        pay_rate_usd: rate,
        pay_unit: PayUnit::Task,
        wallet_address: None,
        kpis: vec![],
    };
    db::insert_worker(pool, None, None, &w).await?;
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let account = pk.implicit_account();
    let now = Utc::now();
    let ch = gates::issue_challenge(pool, w.id, network, &account, &pk.to_near_string(), now).await?;
    let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    gates::verify_challenge(pool, &axom_tools::near_rpc::MockNear::new(), network, ch.id, &sig, now).await?;
    gates::record_tax_form(pool, w.id, TaxFormKind::W9, Some("seed-docuseal-submission")).await?;
    for i in 0..tasks {
        sqlx::query("INSERT INTO axom.tasks (id, objective, required_skills, assignee, status, completed_at, finished_at, objective_check_passed) VALUES ($1, 'Design deliverable', '{visual_design}', $2, 'done', $3, $3, true)")
            .bind(Uuid::new_v4())
            .bind(w.id)
            .bind(period.0 + Duration::hours(2 + i))
            .execute(pool)
            .await?;
    }
    Ok((w.id, account, sk))
}

/// Payroll for three freelancers: USD 5 (sent automatically), USD 60
/// (held for approval), USD 400 (blocked by the monthly cap).
pub async fn payouts(pool: &PgPool, c: &Configs, attestor: &axom_core::authz::AuthorizationKey, price: PriceQuote, network: &str, now: DateTime<Utc>) -> anyhow::Result<PayoutDemo> {
    let period = (now - Duration::days(7), now);
    let (small_w, small_recipient) = ready_freelancer(pool, "Dana (small payout)", Decimal::from(5), 1, period, network).await?;
    let (held_w, _) = ready_freelancer(pool, "Lee (needs approval)", Decimal::from(30), 2, period, network).await?;
    let (big_w, _) = ready_freelancer(pool, "Kai (over the cap)", Decimal::from(100), 4, period, network).await?;
    // Ade is paid in USDC on Base: they sign the destination with their verified NEAR key.
    let (xc_w, _, xc_key) = ready_freelancer_keyed(pool, "Ade (paid in USDC on Base)", Decimal::from(3), 1, period, network).await?;
    let dest = Destination { chain: "base".into(), asset_id: USDC_BASE_ASSET.into(), address: DEMO_BASE_ADDRESS.into() };
    let ch = destinations::issue_challenge(pool, xc_w, network, &dest, Utc::now()).await?;
    let sig = hex::encode(xc_key.sign(ch.message.as_bytes()).to_bytes());
    destinations::verify_challenge(pool, &axom_tools::near_rpc::MockNear::new(), ch.id, &sig, Utc::now()).await?;
    let run = PayrollRun { policy: &c.policy, price: Some(price), fee_near: Decimal::new(1, 3), network, attestor, period };
    let lines = payroll::run_payroll(pool, &run, now).await?;
    let id_of = |w: Uuid| -> anyhow::Result<(Uuid, PayoutStatus)> {
        match &lines.iter().find(|l| l.owed.worker_id == w).context("no payroll line")?.outcome {
            PayrollOutcome::Created { created, .. } => Ok((created.payout_id, created.status)),
            other => anyhow::bail!("unexpected payroll outcome {other:?}"),
        }
    };
    let (small, s1) = id_of(small_w)?;
    let (held, s2) = id_of(held_w)?;
    let (blocked, s3) = id_of(big_w)?;
    let (cross_chain, s4) = id_of(xc_w)?;
    anyhow::ensure!(s1 == PayoutStatus::Approved, "small payout is {s1}");
    anyhow::ensure!(s4 == PayoutStatus::Approved, "cross-chain payout is {s4}");
    anyhow::ensure!(s2 == PayoutStatus::PendingApproval, "held payout is {s2}");
    anyhow::ensure!(s3 == PayoutStatus::CapExceeded, "over-cap payout is {s3}");
    Ok(PayoutDemo { small, small_recipient, held, blocked, cross_chain, cross_chain_destination: dest })
}

#[derive(Debug, Clone, Serialize)]
pub struct FinalReport {
    pub month: chrono::NaiveDate,
    pub cap_usd: Decimal,
    pub committed_usd: Decimal,
    pub reserved_usd: Decimal,
    pub remaining_usd: Decimal,
    pub waiting_for_approval: Vec<Value>,
    pub blocked: Vec<Value>,
    pub sent: Vec<Value>,
    pub fired: Vec<Value>,
    pub interviews: Vec<Value>,
    /// Payouts delivered on another chain through NEAR Intents.
    pub cross_chain: Vec<Value>,
}

pub async fn final_report(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<FinalReport> {
    let u = cap::usage(pool, cap::month_of(now)).await?;
    let rows = |status: &'static str| async move {
        let r: Vec<(Uuid, String, Decimal, String, Option<String>)> =
            sqlx::query_as("SELECT id, recipient, amount_usd, status_reason, tx_hash FROM axom.payouts WHERE status = $1 ORDER BY created_at")
                .bind(status)
                .fetch_all(pool)
                .await?;
        Ok::<Vec<Value>, sqlx::Error>(r.into_iter().map(|x| json!({ "payout_id": x.0, "recipient": x.1, "amount_usd": x.2, "reason": x.3, "tx_hash": x.4 })).collect())
    };
    let mut sent = rows("sent").await?;
    sent.extend(rows("finalized").await?.into_iter().filter(|v| !v["tx_hash"].is_null()));
    let mut blocked = rows("cap_exceeded").await?;
    blocked.extend(rows("blocked").await?);
    let fired: Vec<(String, String)> = sqlx::query_as("SELECT name, role FROM axom.workers WHERE status = 'fired'").fetch_all(pool).await?;
    let interviews: Vec<(Uuid, String, Option<Value>, Option<Value>)> = sqlx::query_as("SELECT id, status, consent, scores FROM axom.meetings ORDER BY created_at").fetch_all(pool).await?;
    Ok(FinalReport {
        month: u.month,
        cap_usd: u.cap_usd,
        committed_usd: u.committed_usd,
        reserved_usd: u.reserved_usd,
        remaining_usd: u.remaining_usd(),
        waiting_for_approval: rows("pending_approval").await?,
        blocked,
        sent,
        fired: fired.into_iter().map(|f| json!({ "name": f.0, "role": f.1 })).collect(),
        cross_chain: {
            type X = (Uuid, String, Decimal, String, String, Option<String>, Option<String>, Option<String>, Option<Value>);
            let r: Vec<X> = sqlx::query_as(
                "SELECT id, status, amount_usd, dest_chain, dest_address, tx_hash, intent_deposit_address, intent_status, intent_tx_hashes
                 FROM axom.payouts WHERE destination_id IS NOT NULL ORDER BY created_at",
            )
            .fetch_all(pool)
            .await?;
            r.into_iter()
                .map(|x| json!({ "payout_id": x.0, "status": x.1, "amount_usd": x.2, "chain": x.3, "address": x.4, "near_deposit_tx": x.5,
                                 "intent_deposit_address": x.6, "intent_status": x.7, "destination_txs": x.8 }))
                .collect()
        },
        interviews: interviews.into_iter().map(|m| json!({ "meeting_id": m.0, "status": m.1, "consent": m.2, "scores": m.3 })).collect(),
    })
}
