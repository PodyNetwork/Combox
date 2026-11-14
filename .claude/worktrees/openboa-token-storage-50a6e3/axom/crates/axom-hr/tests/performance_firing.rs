//! Requires DATABASE_URL. Reviews, firing rule, firing execution, owner
//! notifications, LLM judge independence, and spot checks.

use std::path::Path;

use axom_core::db;
use axom_core::domain::{Actor, PayUnit, Worker, WorkerKind};
use axom_core::state::WorkerStatus;
use axom_core::{Decimal, Uuid};
use axom_hr::notify::{self, NotifyConfig};
use axom_hr::performance::PerformanceConfig;
use axom_hr::reviews::{self, QualityError, Trigger};
use axom_hr::{cap, PolicyConfig};
use axom_tools::mock::MockTools;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn repo(p: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(p)
}

struct Env {
    policy: PolicyConfig,
    perf: PerformanceConfig,
    notify: NotifyConfig,
}

async fn setup(pool: &PgPool) -> Env {
    let policy = PolicyConfig::parse(&std::fs::read_to_string(repo("config/policy.toml")).unwrap()).unwrap();
    cap::seed_cap(pool, &policy).await.unwrap();
    let perf = PerformanceConfig::load(&repo("config/performance.toml")).unwrap();
    let notify = NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "https://admin.example.com".into() };
    Env { policy, perf, notify }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
}

fn day(d: f64) -> DateTime<Utc> {
    t0() + Duration::seconds((d * 86_400.0) as i64)
}

async fn add_worker(pool: &PgPool, name: &str, tools: &[&str], skills: &[&str]) -> Uuid {
    let w = Worker {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: WorkerKind::AiAgent,
        role: "engineer".into(),
        status: WorkerStatus::Active,
        skills: skills.iter().map(|s| s.to_string()).collect(),
        tool_allowlist: tools.iter().map(|s| s.to_string()).collect(),
        pay_rate_usd: dec!(2),
        pay_unit: PayUnit::Task,
        wallet_address: None,
        kpis: vec![],
    };
    db::insert_worker(pool, None, None, &w).await.unwrap();
    sqlx::query("UPDATE axom.workers SET created_at = $2, model = 'worker-model-a' WHERE id = $1")
        .bind(w.id).bind(t0()).execute(pool).await.unwrap();
    w.id
}

/// Insert a finished task. `on_time = false` puts the deadline a day before
/// the finish date.
async fn add_finished(pool: &PgPool, worker: Uuid, at: DateTime<Utc>, done: bool, on_time: bool, cost: Decimal) -> Uuid {
    let id = Uuid::new_v4();
    let finish_date: NaiveDate = at.date_naive();
    let deadline = if on_time { finish_date + Duration::days(1) } else { finish_date - Duration::days(1) };
    sqlx::query(
        "INSERT INTO axom.tasks (id, objective, required_tools, required_skills, deadline, assignee, status, cost_usd, finished_at, completed_at, created_at)
         VALUES ($1, 'work', '{code_repo}', '{software_engineering}', $2, $3, $4, $5, $6, CASE WHEN $7 THEN $6 END, $6)",
    )
    .bind(id).bind(deadline).bind(worker).bind(if done { "done" } else { "failed" }).bind(cost).bind(at).bind(done)
    .execute(pool).await.unwrap();
    id
}

async fn add_open(pool: &PgPool, worker: Uuid, tools: &[&str], skills: &[&str]) -> Uuid {
    let id = Uuid::new_v4();
    let tools: Vec<String> = tools.iter().map(|s| s.to_string()).collect();
    let skills: Vec<String> = skills.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO axom.tasks (id, objective, required_tools, required_skills, assignee, status) VALUES ($1, 'open work', $2, $3, $4, 'assigned')")
        .bind(id).bind(tools).bind(skills).bind(worker).execute(pool).await.unwrap();
    id
}

/// `n` tasks evenly spread over (from, to]: `done` of them completed late, the rest failed.
async fn batch(pool: &PgPool, worker: Uuid, from: f64, to: f64, n: usize, done: usize, on_time: bool) {
    for i in 0..n {
        let at = day(from + (to - from) * (i as f64 + 1.0) / n as f64);
        add_finished(pool, worker, at, i < done, on_time, dec!(1)).await;
    }
}

async fn status(pool: &PgPool, w: Uuid) -> WorkerStatus {
    db::load_worker(pool, w).await.unwrap().unwrap().status
}

async fn run(pool: &PgPool, env: &Env, at: DateTime<Utc>) -> Vec<reviews::ReviewOutcome> {
    reviews::run_due_reviews(pool, &env.policy, &env.perf, &env.notify, at).await.unwrap()
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn ten_completed_over_three_days_does_not_fire(pool: PgPool) {
    let env = setup(&pool).await;
    let w = add_worker(&pool, "fast-but-bad", &["code_repo"], &["software_engineering"]).await;
    batch(&pool, w, 0.0, 1.5, 10, 5, false).await;
    let r1 = run(&pool, &env, day(1.5)).await;
    assert_eq!(r1.len(), 1);
    assert_eq!(r1[0].review.trigger, Trigger::TaskCount);
    assert!(r1[0].review.assessment.objective_below);
    batch(&pool, w, 1.5, 3.0, 10, 5, false).await;
    let r2 = run(&pool, &env, day(3.0)).await;
    assert!(r2[0].review.assessment.objective_below);
    assert!(r2[0].fired.is_none(), "two below periods but only 3 days of data");
    assert_eq!(status(&pool, w).await, WorkerStatus::OnProbation);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn fourteen_days_with_five_completed_does_not_fire(pool: PgPool) {
    let env = setup(&pool).await;
    let w = add_worker(&pool, "slow-and-bad", &["code_repo"], &["software_engineering"]).await;
    batch(&pool, w, 0.0, 6.9, 7, 2, false).await;
    let r1 = run(&pool, &env, day(7.0)).await;
    assert_eq!(r1[0].review.trigger, Trigger::PeriodEnd);
    batch(&pool, w, 7.0, 13.9, 8, 3, false).await;
    let r2 = run(&pool, &env, day(14.0)).await;
    assert!(r2[0].review.assessment.objective_below);
    assert!(r2[0].fired.is_none(), "14 days of data but only 5 completed tasks");
    assert_ne!(status(&pool, w).await, WorkerStatus::Fired);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn two_consecutive_below_periods_fire_with_reassignment_and_notification(pool: PgPool) {
    let env = setup(&pool).await;
    let w = add_worker(&pool, "low-performer", &["code_repo", "postiz"], &["software_engineering", "copywriting"]).await;
    let backup = add_worker(&pool, "backup-engineer", &["code_repo"], &["software_engineering"]).await;
    let coding = add_open(&pool, w, &["code_repo"], &["software_engineering"]).await;
    let social = add_open(&pool, w, &["postiz"], &["copywriting"]).await;

    batch(&pool, w, 0.0, 7.0, 10, 6, false).await;
    let r1 = run(&pool, &env, day(7.0)).await;
    let mine = r1.iter().find(|o| o.review.worker_id == w).unwrap();
    assert!(mine.review.assessment.objective_below);
    assert!(mine.fired.is_none(), "one below-threshold period does not fire");
    assert_eq!(status(&pool, w).await, WorkerStatus::OnProbation);

    // First finish was day 0.7, so 14 days of data are reached on day 14.7.
    batch(&pool, w, 7.0, 14.9, 10, 6, false).await;
    let before = Utc::now();
    let r2 = run(&pool, &env, day(15.0)).await;
    let mine = r2.iter().find(|o| o.review.worker_id == w).unwrap();
    let fired = mine.fired.as_ref().expect("second consecutive below period fires");

    // Status, revoked tools, final review.
    let worker = db::load_worker(&pool, w).await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Fired);
    assert!(worker.tool_allowlist.is_empty());
    assert_eq!(fired.revoked_tools, vec!["code_repo".to_string(), "postiz".to_string()]);
    let (final_trigger,): (String,) = sqlx::query_as("SELECT trigger FROM axom.reviews WHERE id = $1").bind(fired.final_review_id).fetch_one(&pool).await.unwrap();
    assert_eq!(final_trigger, "final");

    // Reassignment: the coding task moves to the backup; the social task has no taker.
    assert_eq!(fired.reopened_tasks.len(), 2);
    assert_eq!(fired.reassigned, vec![(coding, backup)]);
    assert_eq!(fired.hiring_requests.len(), 1);
    let (hr_task,): (Uuid,) = sqlx::query_as("SELECT task_id FROM axom.hiring_requests WHERE id = $1").bind(fired.hiring_requests[0]).fetch_one(&pool).await.unwrap();
    assert_eq!(hr_task, social);

    // Audit record with reason and metrics.
    let events = axom_core::audit::by_action(&pool, "worker.fire").await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, fired.audit_id);
    assert!(events[0].reasoning_summary.contains("2 consecutive review periods"));
    assert_eq!(events[0].inputs["metrics"]["completed"], 12);

    // Owner notification: generated with the firing, within the limit.
    let n = notify::get(&pool, fired.notification_id).await.unwrap().unwrap();
    assert_eq!(n.kind, "worker_fired");
    assert!((n.created_at - before) < Duration::seconds(60));
    assert!(n.body["reason"].as_str().unwrap().contains("consecutive"));
    assert_eq!(n.body["metrics"]["completed"], 12);
    assert_eq!(n.body["audit_link"], format!("https://admin.example.com/audit/{}", fired.audit_id));
    assert!(notify::feed(&pool, 10).await.unwrap().iter().any(|f| f.id == n.id), "on the dashboard feed");

    // Email delivery through SMTP (mock).
    let mailer = MockTools::new();
    let sent = notify::deliver_pending(&pool, &env.policy, &env.notify, &mailer).await.unwrap();
    assert_eq!(sent.sent, vec![n.id]);
    let email = &mailer.calls_to("smtp")[0].args["email"];
    assert_eq!(email["to"][0], "owner@example.com");
    let body = email["body"].as_str().unwrap();
    assert!(body.contains("Reason:") && body.contains("Metrics:") && body.contains(&format!("/audit/{}", fired.audit_id)));
    let n = notify::get(&pool, fired.notification_id).await.unwrap().unwrap();
    assert!(n.email_sent_at.is_some());
    assert!((n.email_sent_at.unwrap() - before) < Duration::seconds(60));
    // Nothing left to send.
    assert!(notify::deliver_pending(&pool, &env.policy, &env.notify, &mailer).await.unwrap().sent.is_empty());

    // The backup is unaffected.
    assert_eq!(status(&pool, backup).await, WorkerStatus::Active);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn low_llm_only_quality_does_not_fire(pool: PgPool) {
    let env = setup(&pool).await;
    let w = add_worker(&pool, "objectively-good", &["code_repo"], &["software_engineering"]).await;
    for period in 0..3 {
        for i in 0..10 {
            let at = day(period as f64 * 7.0 + 0.5 * (i as f64 + 1.0));
            let t = add_finished(&pool, w, at, true, true, dec!(1)).await;
            reviews::record_objective_check(&pool, t, true).await.unwrap();
            reviews::record_llm_quality(&pool, t, "judge-model-b", 0.05).await.unwrap();
        }
        let out = run(&pool, &env, day(period as f64 * 7.0 + 6.0)).await;
        let r = &out[0].review;
        assert!((r.metrics.llm_quality.unwrap() - 0.05).abs() < 1e-9);
        assert!(!r.assessment.objective_below, "{:?}", r.assessment);
        assert!(out[0].fired.is_none());
    }
    assert_eq!(status(&pool, w).await, WorkerStatus::Active);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn llm_judge_must_differ_from_worker_model(pool: PgPool) {
    setup(&pool).await;
    let w = add_worker(&pool, "w", &["code_repo"], &["software_engineering"]).await;
    let t = add_finished(&pool, w, day(1.0), true, true, dec!(1)).await;
    assert!(matches!(reviews::record_llm_quality(&pool, t, "worker-model-a", 0.9).await, Err(QualityError::JudgeNotIndependent(_))));
    assert!(matches!(reviews::record_llm_quality(&pool, t, "judge", 1.5).await, Err(QualityError::OutOfRange)));
    reviews::record_llm_quality(&pool, t, "judge-model-b", 0.9).await.unwrap();
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn spot_checks_sample_reviews_and_report_disagreement(pool: PgPool) {
    let env = setup(&pool).await;
    let w = add_worker(&pool, "w", &["code_repo"], &["software_engineering"]).await;
    for i in 0..400 {
        reviews::store_review(&pool, &env.perf, w, Trigger::PeriodEnd, day(i as f64 * 0.01)).await.unwrap();
    }
    let stats = reviews::spot_check_stats(&pool).await.unwrap();
    assert_eq!(stats.reviews, 400);
    assert!((20..=65).contains(&stats.sampled), "sampled {} of 400", stats.sampled);
    assert_eq!(stats.disagreement_rate, None);

    let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM axom.spot_checks ORDER BY id LIMIT 4").fetch_all(&pool).await.unwrap();
    let admin = Actor::Admin { name: "owner".into(), mfa_verified: true };
    for (i, (id,)) in ids.iter().enumerate() {
        reviews::record_spot_check(&pool, &admin, *id, i != 0, "checked").await.unwrap();
    }
    assert!(reviews::record_spot_check(&pool, &Actor::agent("hr"), ids[0].0, true, "").await.is_err());
    let stats = reviews::spot_check_stats(&pool).await.unwrap();
    assert_eq!((stats.answered, stats.disagreements), (4, 1));
    assert_eq!(stats.disagreement_rate, Some(0.25));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn owner_alerts_still_deliver_under_kill_switch(pool: PgPool) {
    let env = setup(&pool).await;
    notify::enqueue(&pool, "test", "hello", serde_json::json!({ "reason": "r" }), None).await.unwrap();
    sqlx::query("UPDATE axom.system_controls SET kill_switch = true").execute(&pool).await.unwrap();
    let mailer = MockTools::new();
    assert_eq!(notify::deliver_pending(&pool, &env.policy, &env.notify, &mailer).await.unwrap().sent.len(), 1);
    // A misconfigured recipient is refused outright.
    notify::enqueue(&pool, "test", "again", serde_json::json!({}), None).await.unwrap();
    let bad = NotifyConfig { owner_email: String::new(), dashboard_url: "x".into() };
    assert!(notify::deliver_pending(&pool, &env.policy, &bad, &mailer).await.is_err());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn failed_deliveries_are_retried_then_kept_in_feed(pool: PgPool) {
    let env = setup(&pool).await;
    let id = notify::enqueue(&pool, "test", "hello", serde_json::json!({}), None).await.unwrap();
    // A mailer that always fails.
    struct Down;
    #[async_trait::async_trait]
    impl axom_tools::Mailer for Down {
        async fn send(&self, _: &axom_tools::Authorized, _: &axom_tools::Email) -> axom_tools::ToolResult<String> {
            Err(axom_tools::ToolError::Retryable("smtp down".into()))
        }
    }
    for _ in 0..notify::MAX_ATTEMPTS + 2 {
        notify::deliver_pending(&pool, &env.policy, &env.notify, &Down).await.unwrap();
    }
    let n = notify::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(n.attempts, notify::MAX_ATTEMPTS);
    assert!(n.email_sent_at.is_none());
    assert!(notify::feed(&pool, 10).await.unwrap().iter().any(|f| f.id == id));
}
