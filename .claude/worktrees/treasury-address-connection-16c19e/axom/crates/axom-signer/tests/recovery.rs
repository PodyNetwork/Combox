//! Requires DATABASE_URL. Payouts that used to get stuck for good: the
//! signer's own bookkeeping (nonces, unanswered sends, refusals) and payroll
//! picking a period up again after its payout never paid.

mod common;

use axom_core::state::PayoutStatus;
use axom_core::Uuid;
use axom_hr::payroll::{self, PayrollOutcome, PayrollRun};
use axom_hr::{cap, notify, payouts};
use axom_signer::{SendResult, Signer};
use axom_tools::ToolError;
use chrono::Duration;
use common::*;
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn signer<'a>(pool: &'a PgPool, f: &'a Fixture, t: &'a LocalTransit) -> Signer<'a> {
    Signer { pool, policy: &f.policy, cfg: &f.cfg, transit: t, near: &f.near, notify: &f.notify, intents: None }
}

async fn status(pool: &PgPool, id: Uuid) -> PayoutStatus {
    payouts::status(pool, id).await.unwrap()
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>(sql).fetch_one(pool).await.unwrap().0
}

async fn run(pool: &PgPool, f: &Fixture, at: chrono::DateTime<chrono::Utc>) -> Vec<payroll::PayrollLine> {
    let price = Some(axom_hr::PriceQuote { usd_per_near: dec!(4), fetched_at: at - Duration::minutes(1) });
    let run = PayrollRun { policy: &f.policy, price, fee_near: dec!(0.001), network: "testnet", attestor: &f.attestor, period: period() };
    payroll::run_payroll(pool, &run, at).await.unwrap()
}

fn outcome(lines: &[payroll::PayrollLine], worker: Uuid) -> PayrollOutcome {
    lines.iter().find(|l| l.owed.worker_id == worker).unwrap().outcome.clone()
}

/// The node reports the treasury key's nonce a couple of blocks behind, so
/// the second payout of a cycle must not reuse the first one's.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn payouts_sent_back_to_back_use_different_nonces(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    f.near.lag_views(true);
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
        ids.push(f.payroll(&pool, w).await);
    }
    let out = signer(&pool, &f, &t).process_due(now()).await.unwrap();
    assert_eq!(out.len(), 3);
    for (id, r) in &out {
        assert!(matches!(r, SendResult::Sent { .. }), "{id}: {r:?}");
    }
    let mut nonces: Vec<u64> = f.near.sent().iter().map(|tx| tx.nonce).collect();
    nonces.sort();
    assert_eq!(nonces, vec![101, 102, 103]);
}

/// The send went through but its answer was lost. Nothing is retried blindly
/// and nothing is sent twice; and an authorization that expired in the
/// meantime does not hide that the money moved.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_send_whose_answer_was_lost_is_found_on_chain(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    f.near.lose_next_answer();
    let s = signer(&pool, &f, &t);
    assert!(matches!(s.process_due(now()).await.unwrap()[0].1, SendResult::Failed { final_attempt: false, .. }));
    assert_eq!(f.near.sent().len(), 1, "it did land");

    // Two days later the automatic attestation has long expired.
    let later = now() + Duration::days(2);
    let out = s.process_due(later).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
    assert_eq!(f.near.sent().len(), 1, "not sent a second time");
    assert!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd() > dec!(0), "counted against the cap");
}

/// A refused payout is looked at every cycle; it is recorded and reported
/// once, not once per cycle.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_refusal_is_reported_once(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    sqlx::query("UPDATE axom.system_controls SET kill_switch = true").execute(&pool).await.unwrap();
    let s = signer(&pool, &f, &t);
    for i in 0..6 {
        let out = s.process_due(now() + Duration::seconds(10 * i)).await.unwrap();
        assert!(matches!(out[0].1, SendResult::Rejected { .. }), "{out:?}");
    }
    assert_eq!(notify::count_kind(&pool, "signer_rejected").await.unwrap(), 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM axom.audit_log WHERE action = 'signer.sign' AND outcome = 'rejected'").await, 1);
    // Switched off again: the payout goes out.
    sqlx::query("UPDATE axom.system_controls SET kill_switch = false").execute(&pool).await.unwrap();
    assert!(matches!(s.process_due(now() + Duration::minutes(2)).await.unwrap()[0].1, SendResult::Sent { .. }));
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
}

/// A treasury that isn't funded yet pauses payouts; it doesn't use up their
/// retries and fail them for good minutes after the key was created.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn an_unfunded_treasury_pauses_instead_of_failing(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, LocalTransit::new().public()).await; // access key registered for another key
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    let s = signer(&pool, &f, &t);
    for i in 0..8 {
        let out = s.process_due(now() + Duration::minutes(i)).await.unwrap();
        assert!(matches!(out[0].1, SendResult::Paused { .. }), "{out:?}");
    }
    let (attempts,): (i32,) = sqlx::query_as("SELECT attempts FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((attempts, status(&pool, id).await), (0, PayoutStatus::Approved));
    assert_eq!(count(&pool, "SELECT count(*) FROM axom.audit_log WHERE action = 'signer.sign' AND outcome = 'paused'").await, 1, "one record, not one per cycle");
    // Funded: it goes out.
    f.near.add_access_key(&t.public().implicit_account(), t.public(), 7);
    assert!(matches!(s.process_due(now() + Duration::minutes(9)).await.unwrap()[0].1, SendResult::Sent { .. }));
}

/// One payout the signer can't process doesn't stop the ones after it.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn one_broken_payout_does_not_block_the_rest(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w1, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let first = f.payroll(&pool, w1).await;
    let (w2, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let second = f.payroll(&pool, w2).await;
    // The older payout carries a stored transaction that can't be decoded.
    sqlx::query("UPDATE axom.payouts SET tx_hash = 'abc', signed_tx = '%%not-base64%%', created_at = created_at - interval '1 hour' WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    let out = signer(&pool, &f, &t).process_due(now()).await.unwrap();
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].0, second);
    assert_eq!(status(&pool, second).await, PayoutStatus::Sent);
}

/// An approved payout whose attestation is missing or has expired is made
/// sendable again while it is recent, and given up cleanly once it is not.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn authorizations_are_repaired_renewed_or_given_up(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let s = signer(&pool, &f, &t);

    // Missing (a crash between creating the payout and attesting it).
    let (w1, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let missing = f.payroll(&pool, w1).await;
    sqlx::query("DELETE FROM axom.payout_authorizations WHERE payout_id = $1").bind(missing).execute(&pool).await.unwrap();
    assert!(s.process_due(now()).await.unwrap().is_empty(), "nothing to send without an authorization");
    let r = payroll::refresh_authorizations(&pool, &f.policy, &f.attestor, now()).await.unwrap();
    assert_eq!((r.issued.as_slice(), r.renewed.len(), r.abandoned.len()), ([missing].as_slice(), 0, 0));
    assert!(matches!(s.process_due(now()).await.unwrap()[0].1, SendResult::Sent { .. }));

    // Expired while the vault was sealed for two days: signed again.
    let (w2, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let expired = f.payroll(&pool, w2).await;
    let in_two_days = now() + Duration::days(2);
    assert!(matches!(s.process_due(in_two_days).await.unwrap()[0].1, SendResult::Rejected { .. }));
    let r = payroll::refresh_authorizations(&pool, &f.policy, &f.attestor, in_two_days).await.unwrap();
    assert_eq!((r.issued.len(), r.renewed.as_slice(), r.abandoned.len()), (0, [expired].as_slice(), 0));
    assert!(matches!(s.process_due(in_two_days).await.unwrap()[0].1, SendResult::Sent { .. }));

    // Expired and older than an approval may be: given up, reservation
    // released, and the period is paid with a new payout.
    let (w3, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let stale = f.payroll(&pool, w3).await;
    let used_before = cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd();
    let much_later = now() + Duration::days(9);
    let r = payroll::refresh_authorizations(&pool, &f.policy, &f.attestor, much_later).await.unwrap();
    assert_eq!(r.abandoned, vec![stale]);
    assert_eq!(status(&pool, stale).await, PayoutStatus::Failed);
    assert!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd() < used_before, "reservation released");
    assert!(s.process_due(much_later).await.unwrap().is_empty(), "the signer leaves it alone");
    let PayrollOutcome::Created { created, attested: true } = outcome(&run(&pool, &f, much_later).await, w3) else { panic!() };
    assert_ne!(created.payout_id, stale);
    assert!(matches!(s.process_due(much_later).await.unwrap()[0].1, SendResult::Sent { .. }));
}

/// A payout that failed for good no longer blocks its pay period: payroll
/// pays it with a new payout, up to a limit.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_period_is_paid_again_after_a_final_failure_up_to_a_limit(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let s = signer(&pool, &f, &t);
    let mut made = Vec::new();
    for round in 0..payroll::MAX_PAYOUTS_PER_PERIOD {
        let at = now() + Duration::hours(round as i64);
        let PayrollOutcome::Created { created, .. } = outcome(&run(&pool, &f, at).await, w) else { panic!("round {round}") };
        made.push(created.payout_id);
        // While it is live, the period is taken.
        assert!(matches!(outcome(&run(&pool, &f, at).await, w), PayrollOutcome::AlreadyPaid { .. }));
        // The node rejects the transaction outright: final.
        f.near.fail_next_sends(vec![ToolError::Permanent("INVALID_TRANSACTION InvalidAccessKeyError".into())]);
        let out = s.process_due(at).await.unwrap();
        assert!(matches!(out[0].1, SendResult::Failed { final_attempt: true, .. }), "{out:?}");
    }
    made.dedup();
    assert_eq!(made.len(), payroll::MAX_PAYOUTS_PER_PERIOD, "a new payout each time");
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(0), "nothing left reserved");
    // After the limit payroll stops and leaves it to the owner.
    let PayrollOutcome::Waiting { reason, .. } = outcome(&run(&pool, &f, now() + Duration::days(1)).await, w) else { panic!() };
    assert!(reason.contains("needs the owner"), "{reason}");
    assert!(f.near.sent().is_empty());
}

/// Over the cap: one record and one alert, then payroll waits until the
/// payout fits, and pays it when the cap is raised.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_period_over_the_cap_is_paid_once_it_fits(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(600), 1).await;
    let PayrollOutcome::Created { created, .. } = outcome(&run(&pool, &f, now()).await, w) else { panic!() };
    assert_eq!(created.status, PayoutStatus::CapExceeded);
    for hour in 1..4 {
        let PayrollOutcome::Waiting { payout_id, reason } = outcome(&run(&pool, &f, now() + Duration::hours(hour)).await, w) else { panic!() };
        assert_eq!(payout_id, created.payout_id);
        assert!(reason.contains("cap"), "{reason}");
    }
    assert_eq!(count(&pool, "SELECT count(*) FROM axom.payouts").await, 1, "no new record each hour");
    assert_eq!(notify::count_kind(&pool, "cap_exceeded").await.unwrap(), 1, "no new alert each hour");

    sqlx::query("UPDATE axom.cap_settings SET cap_usd = 1000").execute(&pool).await.unwrap();
    let PayrollOutcome::Created { created: again, .. } = outcome(&run(&pool, &f, now() + Duration::hours(5)).await, w) else { panic!() };
    assert_eq!(again.status, PayoutStatus::PendingApproval, "600 USD still needs an admin");
    assert!(matches!(outcome(&run(&pool, &f, now() + Duration::hours(6)).await, w), PayrollOutcome::AlreadyPaid { .. }));
}

/// Payroll covers every recent week with unpaid freelancer work, not only
/// the last one.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn payroll_looks_back_for_unpaid_weeks(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 0).await;
    let latest = payroll::last_complete_week(now());
    assert_eq!(payroll::periods_to_run(&pool, now()).await.unwrap(), vec![latest], "always the last complete week");

    let three_weeks_ago = latest.0 - Duration::weeks(2) + Duration::days(2);
    let too_old = latest.0 - Duration::weeks(payroll::LOOKBACK_WEEKS + 2);
    let this_week = latest.1 + Duration::hours(1);
    for at in [three_weeks_ago, too_old, this_week] {
        sqlx::query("INSERT INTO axom.tasks (id, objective, assignee, status, completed_at, finished_at) VALUES ($1,'t',$2,'done',$3,$3)")
            .bind(Uuid::new_v4())
            .bind(w)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
    }
    let periods = payroll::periods_to_run(&pool, now()).await.unwrap();
    let start = latest.0 - Duration::weeks(2);
    assert_eq!(periods, vec![(start, start + Duration::days(7)), latest], "the week with unpaid work, oldest first; not the unfinished week, not beyond the lookback");

    // Running that week pays it.
    let price = Some(axom_hr::PriceQuote { usd_per_near: dec!(4), fetched_at: now() - Duration::minutes(1) });
    let run = PayrollRun { policy: &f.policy, price, fee_near: dec!(0.001), network: "testnet", attestor: &f.attestor, period: periods[0] };
    let lines = payroll::run_payroll(&pool, &run, now()).await.unwrap();
    assert!(matches!(outcome(&lines, w), PayrollOutcome::Created { attested: true, .. }));
}

/// The stored transaction's nonce was used by something else, so it can
/// never land. That is not a final failure: once two looks a backoff apart
/// agree it isn't on chain, a new transaction is signed.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_transaction_whose_nonce_was_taken_is_signed_again(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    let s = signer(&pool, &f, &t);
    f.near.fail_next_sends(vec![ToolError::Retryable("rpc timeout".into())]);
    let mut clock = now();
    let step = |r: &SendResult| match r {
        SendResult::Failed { next_attempt_at: Some(next), final_attempt: false, .. } => *next,
        other => panic!("{other:?}"),
    };
    clock = step(&s.process_due(clock).await.unwrap()[0].1);
    // Nonce 101 (the one just signed) is used by another transaction.
    f.near.add_access_key(&t.public().implicit_account(), t.public(), 101);
    clock = step(&s.process_due(clock).await.unwrap()[0].1);
    let (kept,): (Option<String>,) = sqlx::query_as("SELECT tx_hash FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(kept.is_some(), "kept after the first look: the node answering may be behind");
    clock = step(&s.process_due(clock).await.unwrap()[0].1);
    let (dropped,): (Option<String>,) = sqlx::query_as("SELECT tx_hash FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(dropped.is_none(), "dropped after the second");
    let out = s.process_due(clock).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    let sent = f.near.sent();
    assert_eq!((sent.len(), sent[0].nonce), (1, 102));
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
}
