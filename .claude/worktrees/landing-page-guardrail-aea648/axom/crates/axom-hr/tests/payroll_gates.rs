//! Requires DATABASE_URL. Payroll math, address and tax gates, automatic
//! attestations, and human approvals.

use std::path::Path;

use axom_core::authz::AuthorizationKey;
use axom_core::db;
use axom_core::domain::{PayUnit, Worker, WorkerKind};
use axom_core::near::PublicKey;
use axom_core::state::{PayoutStatus, WorkerStatus};
use axom_core::Uuid;
use axom_hr::approvals::{self, AdminSession, ApprovalError};
use axom_hr::gates::{self, GateError};
use axom_hr::payroll::{self, PayrollOutcome, PayrollRun};
use axom_hr::{cap, payouts, PolicyConfig, PriceQuote};
use axom_tools::near_rpc::MockNear;
use axom_tools::TaxFormKind;
use chrono::{DateTime, Duration, TimeZone, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn policy() -> PolicyConfig {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml");
    PolicyConfig::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn period() -> (DateTime<Utc>, DateTime<Utc>) {
    (Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2026, 11, 8, 0, 0, 0).unwrap())
}

fn now() -> DateTime<Utc> {
    period().1 + Duration::hours(1)
}

fn quote() -> Option<PriceQuote> {
    Some(PriceQuote { usd_per_near: dec!(4), fetched_at: now() - Duration::minutes(1) })
}

async fn worker(pool: &PgPool, kind: WorkerKind, rate: Decimal, unit: PayUnit) -> Uuid {
    let w = Worker {
        id: Uuid::new_v4(),
        name: "w".into(),
        kind,
        role: "r".into(),
        status: WorkerStatus::Active,
        skills: Default::default(),
        tool_allowlist: Default::default(),
        pay_rate_usd: rate,
        pay_unit: unit,
        wallet_address: None,
        kpis: vec![],
    };
    db::insert_worker(pool, None, None, &w).await.unwrap();
    w.id
}

async fn done_task(pool: &PgPool, worker: Uuid, at: DateTime<Utc>, hours: Option<Decimal>) {
    sqlx::query("INSERT INTO axom.tasks (id, objective, assignee, status, completed_at, finished_at, hours_worked) VALUES ($1,'t',$2,'done',$3,$3,$4)")
        .bind(Uuid::new_v4()).bind(worker).bind(at).bind(hours).execute(pool).await.unwrap();
}

/// Verify an implicit-account address for the worker; returns (key, account).
async fn verify_address(pool: &PgPool, worker: Uuid) -> (SigningKey, String) {
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let account = pk.implicit_account();
    let ch = gates::issue_challenge(pool, worker, "testnet", &account, &pk.to_near_string(), now()).await.unwrap();
    let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    gates::verify_challenge(pool, &MockNear::new(), "testnet", ch.id, &sig, now()).await.unwrap();
    (sk, account)
}

async fn setup(pool: &PgPool) -> PolicyConfig {
    let p = policy();
    cap::seed_cap(pool, &p).await.unwrap();
    p
}

fn run<'a>(p: &'a PolicyConfig, key: &'a AuthorizationKey) -> PayrollRun<'a> {
    PayrollRun { policy: p, price: quote(), fee_near: dec!(0.001), network: "testnet", attestor: key, period: period() }
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn payroll_math_is_completed_work_times_rate(pool: PgPool) {
    setup(&pool).await;
    let (start, end) = period();
    let per_task = worker(&pool, WorkerKind::AiAgent, dec!(2.5), PayUnit::Task).await;
    for d in [0, 2, 6] {
        done_task(&pool, per_task, start + Duration::days(d), None).await;
    }
    done_task(&pool, per_task, end, None).await; // next period
    done_task(&pool, per_task, start - Duration::seconds(1), None).await; // previous period
    let owed = payroll::compute_owed(&pool, per_task, start, end).await.unwrap();
    assert_eq!((owed.units, owed.amount_usd), (dec!(3), dec!(7.5)));

    let hourly = worker(&pool, WorkerKind::HumanFreelancer, dec!(20), PayUnit::Hour).await;
    done_task(&pool, hourly, start + Duration::days(1), Some(dec!(2.5))).await;
    done_task(&pool, hourly, start + Duration::days(3), Some(dec!(1.25))).await;
    let owed = payroll::compute_owed(&pool, hourly, start, end).await.unwrap();
    assert_eq!((owed.units, owed.amount_usd), (dec!(3.75), dec!(75)));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn missing_tax_form_or_unverified_address_blocks_first_payout(pool: PgPool) {
    let p = setup(&pool).await;
    let key = AuthorizationKey::generate();
    let w = worker(&pool, WorkerKind::HumanFreelancer, dec!(10), PayUnit::Task).await;
    done_task(&pool, w, period().0 + Duration::days(1), None).await;

    // Neither gate.
    let lines = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    let PayrollOutcome::Blocked { payout_id, reasons } = &lines[0].outcome else { panic!("{:?}", lines[0].outcome) };
    assert_eq!(reasons.len(), 2);
    assert_eq!(payouts::status(&pool, *payout_id).await.unwrap(), PayoutStatus::Blocked);
    let (reason,): (String,) = sqlx::query_as("SELECT status_reason FROM axom.payouts WHERE id = $1").bind(payout_id).fetch_one(&pool).await.unwrap();
    assert!(reason.contains("verified NEAR address") && reason.contains("W-9"));
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(0), "blocked payouts reserve nothing");

    // Re-running while still blocked updates the same record.
    let again = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    let PayrollOutcome::Blocked { payout_id: again_id, .. } = &again[0].outcome else { panic!() };
    assert_eq!(again_id, payout_id);

    // Address verified, tax form still missing.
    verify_address(&pool, w).await;
    let lines = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    let PayrollOutcome::Blocked { reasons, .. } = &lines[0].outcome else { panic!() };
    assert_eq!(reasons, &vec![gates::GateBlock::TaxFormMissing.reason().to_string()]);

    // Tax form only (new worker): blocked on address.
    let w2 = worker(&pool, WorkerKind::HumanFreelancer, dec!(10), PayUnit::Task).await;
    done_task(&pool, w2, period().0 + Duration::days(1), None).await;
    gates::record_tax_form(&pool, w2, TaxFormKind::W8Ben, Some("sub_1")).await.unwrap();

    // Both gates for w: the payout is created and attested.
    gates::record_tax_form(&pool, w, TaxFormKind::W9, Some("sub_2")).await.unwrap();
    let lines = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    let line_w = lines.iter().find(|l| l.owed.worker_id == w).unwrap();
    let PayrollOutcome::Created { created, attested } = &line_w.outcome else { panic!("{:?}", line_w.outcome) };
    assert_eq!(created.status, PayoutStatus::Approved);
    assert!(attested);
    let line_w2 = lines.iter().find(|l| l.owed.worker_id == w2).unwrap();
    let PayrollOutcome::Blocked { reasons, .. } = &line_w2.outcome else { panic!() };
    assert_eq!(reasons, &vec![gates::GateBlock::AddressUnverified.reason().to_string()]);

    // Re-running does not pay twice.
    let lines = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    assert!(matches!(lines.iter().find(|l| l.owed.worker_id == w).unwrap().outcome, PayrollOutcome::AlreadyPaid { .. }));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn payout_records_near_amount_rate_and_attestation(pool: PgPool) {
    let p = setup(&pool).await;
    let key = AuthorizationKey::generate();
    let small = worker(&pool, WorkerKind::AiAgent, dec!(10), PayUnit::Task).await;
    let big = worker(&pool, WorkerKind::AiAgent, dec!(30), PayUnit::Task).await;
    for w in [small, big] {
        verify_address(&pool, w).await;
        gates::record_tax_form(&pool, w, TaxFormKind::W9, None).await.unwrap();
        for d in 0..2 {
            done_task(&pool, w, period().0 + Duration::days(d), None).await;
        }
    }
    let lines = payroll::run_payroll(&pool, &run(&p, &key), now()).await.unwrap();
    let get = |w: Uuid| lines.iter().find(|l| l.owed.worker_id == w).unwrap().outcome.clone();
    let PayrollOutcome::Created { created: c_small, attested: true } = get(small) else { panic!() };
    let PayrollOutcome::Created { created: c_big, attested: false } = get(big) else { panic!() };
    assert_eq!(c_big.status, PayoutStatus::PendingApproval, "60 USD needs approval");

    let (yocto, rate, fee, ver, res): (String, Decimal, Decimal, String, Option<Uuid>) = sqlx::query_as(
        "SELECT amount_yocto::text, usd_per_near, fee_usd, policy_version, reservation_id FROM axom.payouts WHERE id = $1",
    )
    .bind(c_small.payout_id).fetch_one(&pool).await.unwrap();
    assert_eq!(yocto, "5000000000000000000000000", "20 USD at 4 USD/NEAR = 5 NEAR");
    assert_eq!(rate, dec!(4));
    assert_eq!(fee, dec!(0.004));
    assert_eq!(ver, p.version());
    assert!(res.is_some());
    let (kind,): (String,) = sqlx::query_as("SELECT kind FROM axom.payout_authorizations WHERE payout_id = $1").bind(c_small.payout_id).fetch_one(&pool).await.unwrap();
    assert_eq!(kind, "auto_attestation");
    // A pending payout cannot get an automatic attestation.
    assert!(payroll::issue_auto_attestation(&pool, &p, &key, c_big.payout_id, now()).await.is_err());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn address_challenge_rules(pool: PgPool) {
    setup(&pool).await;
    let w = worker(&pool, WorkerKind::HumanFreelancer, dec!(10), PayUnit::Task).await;
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let other = SigningKey::generate(&mut rand::rngs::OsRng);
    let near = MockNear::new();

    // Implicit account must match its key.
    let wrong_account = PublicKey(other.verifying_key().to_bytes()).implicit_account();
    assert!(matches!(gates::issue_challenge(&pool, w, "testnet", &wrong_account, &pk.to_near_string(), now()).await, Err(GateError::BadAddress(_))));

    // Wrong signer.
    let ch = gates::issue_challenge(&pool, w, "testnet", &pk.implicit_account(), &pk.to_near_string(), now()).await.unwrap();
    let bad = hex::encode(other.sign(ch.message.as_bytes()).to_bytes());
    assert!(matches!(gates::verify_challenge(&pool, &near, "testnet", ch.id, &bad, now()).await, Err(GateError::BadSignature)));
    // Expired.
    let good = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    assert!(matches!(gates::verify_challenge(&pool, &near, "testnet", ch.id, &good, now() + Duration::hours(1)).await, Err(GateError::BadChallenge(_))));
    // Valid, then single-use.
    gates::verify_challenge(&pool, &near, "testnet", ch.id, &good, now()).await.unwrap();
    assert!(matches!(gates::verify_challenge(&pool, &near, "testnet", ch.id, &good, now()).await, Err(GateError::BadChallenge(_))));
    assert!(gates::is_verified_address(&pool, w, "testnet", &pk.implicit_account()).await.unwrap());
    assert!(!gates::is_verified_address(&pool, w, "mainnet", &pk.implicit_account()).await.unwrap());

    // Named account: the key must be a full-access key on chain.
    let ch = gates::issue_challenge(&pool, w, "testnet", "alice.testnet", &pk.to_near_string(), now()).await.unwrap();
    let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    assert!(matches!(gates::verify_challenge(&pool, &near, "testnet", ch.id, &sig, now()).await, Err(GateError::KeyNotOnAccount(_))));
    near.add_access_key("alice.testnet", pk, 1);
    let ch = gates::issue_challenge(&pool, w, "testnet", "alice.testnet", &pk.to_near_string(), now()).await.unwrap();
    let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    gates::verify_challenge(&pool, &near, "testnet", ch.id, &sig, now()).await.unwrap();
}

async fn add_admin(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1, 'owner@example.com', 'Owner')").bind(id).execute(pool).await.unwrap();
    id
}

fn session(user_id: Uuid, mfa: bool) -> AdminSession {
    AdminSession { user_id, email: "owner@example.com".into(), session_id: "sess-1".into(), mfa_verified: mfa }
}

async fn pending_payout(pool: &PgPool, p: &PolicyConfig) -> Uuid {
    let req = payouts::PayoutRequest { recipient: "big.testnet".into(), amount_usd: dec!(75), ..Default::default() };
    let c = payouts::create_payout(pool, p, &axom_core::domain::Actor::System, &req, quote(), now()).await.unwrap();
    assert_eq!(c.status, PayoutStatus::PendingApproval);
    c.payout_id
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn approvals_need_mfa_and_an_active_admin(pool: PgPool) {
    let p = setup(&pool).await;
    let key = AuthorizationKey::generate();
    let admin = add_admin(&pool).await;
    let id = pending_payout(&pool, &p).await;

    let err = approvals::approve_payout(&pool, &p, &key, &session(admin, false), id, now()).await.unwrap_err();
    assert!(matches!(err, ApprovalError::MfaRequired));
    let err = approvals::approve_payout(&pool, &p, &key, &session(Uuid::new_v4(), true), id, now()).await.unwrap_err();
    assert!(matches!(err, ApprovalError::NotAdmin));
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::PendingApproval);
    let denied = axom_core::audit::by_action(&pool, "payout.approve").await.unwrap();
    assert_eq!(denied.iter().filter(|e| e.outcome == "denied").count(), 2);

    let approval_id = approvals::approve_payout(&pool, &p, &key, &session(admin, true), id, now()).await.unwrap();
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::Approved);
    let (kind, payload): (String, serde_json::Value) =
        sqlx::query_as("SELECT kind, payload FROM axom.payout_authorizations WHERE payout_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(kind, "human_approval");
    assert_eq!(payload["approval_id"], approval_id.to_string());
    assert_eq!(payload["approver"], "Owner");
    // Cannot approve twice.
    assert!(approvals::approve_payout(&pool, &p, &key, &session(admin, true), id, now()).await.is_err());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn rejection_releases_reservation(pool: PgPool) {
    let p = setup(&pool).await;
    let admin = add_admin(&pool).await;
    let id = pending_payout(&pool, &p).await;
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(75));
    approvals::reject_payout(&pool, &p, &session(admin, true), id, "not owed", now()).await.unwrap();
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::Rejected);
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(0));
}
