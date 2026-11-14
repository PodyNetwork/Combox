//! Requires DATABASE_URL. Signer authorization checks, sending, sealing,
//! retries, and approvals, against a local transit key and an in-memory
//! NEAR that validates signatures like a node.

mod common;

use axom_core::state::PayoutStatus;
use axom_core::Uuid;
use axom_hr::approvals::{self, AdminSession};
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

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn under_threshold_payout_is_sent_and_finalized(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, account) = f.ready_worker(&pool, dec!(10), 2).await;
    let id = f.payroll(&pool, w).await;
    assert_eq!(status(&pool, id).await, PayoutStatus::Approved);

    let s = signer(&pool, &f, &t);
    let out = s.process_due(now()).await.unwrap();
    let SendResult::Sent { tx_hash } = &out[0].1 else { panic!("{out:?}") };
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);

    // The transfer NEAR accepted: treasury implicit account -> verified recipient, 5 NEAR.
    let sent = f.near.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].signer_id, t.public().implicit_account());
    assert_eq!(sent[0].receiver_id, account);
    assert_eq!(sent[0].transfer_deposit(), Some(5_000_000_000_000_000_000_000_000));
    assert_eq!(sent[0].nonce, 101);
    assert_eq!(bs58::encode(sent[0].signing_hash()).into_string(), *tx_hash);

    // Cap committed, then finalized.
    let u = cap::usage(&pool, cap::month_of(now())).await.unwrap();
    assert_eq!(u.reserved_usd, dec!(0));
    assert!(u.committed_usd > dec!(20));
    assert_eq!(s.finalize_sent(now()).await.unwrap(), vec![id]);
    assert_eq!(status(&pool, id).await, PayoutStatus::Finalized);
    // Nothing left to do.
    assert!(s.process_due(now()).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn large_payout_waits_for_dashboard_approval(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(30), 2).await;
    let id = f.payroll(&pool, w).await;
    assert_eq!(status(&pool, id).await, PayoutStatus::PendingApproval);
    let s = signer(&pool, &f, &t);
    assert!(s.process_due(now()).await.unwrap().is_empty(), "pending payouts are not picked up");
    // Even asking for it directly is refused.
    assert!(matches!(s.send_payout(id, now()).await.unwrap(), SendResult::Rejected { .. }));
    assert!(f.near.sent().is_empty());

    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'o@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let session = AdminSession { user_id: admin, email: "o@example.com".into(), session_id: "s".into(), mfa_verified: true };
    approvals::approve_payout(&pool, &f.policy, &f.approver, &session, id, now()).await.unwrap();
    let out = s.process_due(now()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    assert_eq!(f.near.sent().len(), 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn signer_rejects_payout_without_valid_authorization(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let s = signer(&pool, &f, &t);

    // 1. No authorization at all.
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    sqlx::query("DELETE FROM axom.payout_authorizations WHERE payout_id = $1").bind(id).execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = s.send_payout(id, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("no authorization"), "{reason}");

    // 2. Tampered authorization (amount edited in the database).
    let (w2, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id2 = f.payroll(&pool, w2).await;
    sqlx::query("UPDATE axom.payout_authorizations SET payload = jsonb_set(payload, '{amount_yocto}', '\"9000000000000000000000000\"') WHERE payout_id = $1")
        .bind(id2).execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = s.send_payout(id2, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("authorization invalid"), "{reason}");

    // 3. Signed by the wrong key (an attestation forged with the approval key).
    let (w3, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id3 = f.payroll(&pool, w3).await;
    let rogue = axom_core::authz::AuthorizationKey::generate();
    let (payload,): (serde_json::Value,) = sqlx::query_as("SELECT payload FROM axom.payout_authorizations WHERE payout_id = $1").bind(id3).fetch_one(&pool).await.unwrap();
    let forged = rogue.sign(serde_json::from_value(payload).unwrap());
    sqlx::query("UPDATE axom.payout_authorizations SET signature = $2 WHERE payout_id = $1").bind(id3).bind(&forged.signature).execute(&pool).await.unwrap();
    assert!(matches!(s.send_payout(id3, now()).await.unwrap(), SendResult::Rejected { .. }));

    assert!(f.near.sent().is_empty(), "nothing was signed or sent");
    assert_eq!(status(&pool, id).await, PayoutStatus::Approved);
    assert!(notify::count_kind(&pool, "signer_rejected").await.unwrap() >= 3, "owner alerted");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn signer_rejects_unverified_recipient_and_amount_mismatch(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let s = signer(&pool, &f, &t);

    // Recipient no longer a verified address.
    let (w, account) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    sqlx::query("UPDATE axom.worker_addresses SET revoked_at = now() WHERE account_id = $1").bind(&account).execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = s.send_payout(id, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("not a verified address"), "{reason}");

    // Recipient swapped in the payout record.
    let (w2, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id2 = f.payroll(&pool, w2).await;
    sqlx::query("UPDATE axom.payouts SET recipient = 'attacker.testnet' WHERE id = $1").bind(id2).execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = s.send_payout(id2, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("does not match authorization"), "{reason}");

    // Amount differs from the authorization.
    let (w3, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id3 = f.payroll(&pool, w3).await;
    sqlx::query("UPDATE axom.payouts SET amount_yocto = amount_yocto * 10 WHERE id = $1").bind(id3).execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = s.send_payout(id3, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("does not match authorization"), "{reason}");

    assert!(f.near.sent().is_empty());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn sealed_transit_pauses_and_nothing_is_marked_sent(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    t.seal(true);
    let s = signer(&pool, &f, &t);
    let out = s.process_due(now()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Paused { .. }), "{out:?}");
    assert_eq!(status(&pool, id).await, PayoutStatus::Approved);
    let (attempts, tx): (i32, Option<String>) = sqlx::query_as("SELECT attempts, tx_hash FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((attempts, tx), (0, None), "a pause is not an attempt and nothing was signed");
    assert!(f.near.sent().is_empty());
    // Unsealed: proceeds.
    t.seal(false);
    assert!(matches!(s.process_due(now()).await.unwrap()[0].1, SendResult::Sent { .. }));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn retries_back_off_then_alert_after_final_attempt(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    f.near.fail_next_sends((0..5).map(|_| ToolError::Retryable("rpc timeout".into())).collect());
    let s = signer(&pool, &f, &t);

    let mut clock = now();
    let mut delays = Vec::new();
    for attempt in 1..=5 {
        let out = s.process_due(clock).await.unwrap();
        assert_eq!(out.len(), 1, "attempt {attempt} should run");
        let SendResult::Failed { attempt: a, next_attempt_at, final_attempt, .. } = out[0].1.clone() else { panic!("{out:?}") };
        assert_eq!(a, attempt);
        // A transaction was signed on the first attempt and no send was ever
        // answered, so running out of retries is not the end yet.
        assert!(!final_attempt);
        let next = next_attempt_at.expect("every attempt schedules another look");
        delays.push((next - clock).num_seconds());
        // Not retried before the backoff elapses.
        assert!(s.process_due(next - Duration::seconds(1)).await.unwrap().is_empty());
        clock = next;
    }
    // Four backoffs, then one wait until the signed transaction can no longer land.
    assert_eq!(delays, vec![30, 60, 120, 240, axom_signer::TX_OUTCOME_WAIT_HOURS * 3600]);
    assert_eq!(status(&pool, id).await, PayoutStatus::Failed);
    assert_eq!(notify::count_kind(&pool, "payout_failed").await.unwrap(), 0, "not reported as failed while it may still land");
    assert_eq!(notify::count_kind(&pool, "payout_outcome_unknown").await.unwrap(), 1, "the owner is told it is unconfirmed");
    assert!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd() > dec!(0), "reservation kept while the outcome is unknown");

    // A day on, the transaction has expired without landing: now it is final.
    f.near.expire_pending();
    let out = s.process_due(clock).await.unwrap();
    let SendResult::Failed { final_attempt, next_attempt_at, .. } = out[0].1.clone() else { panic!("{out:?}") };
    assert!(final_attempt && next_attempt_at.is_none());
    assert_eq!(status(&pool, id).await, PayoutStatus::Failed);
    assert!(s.process_due(clock + Duration::days(1)).await.unwrap().is_empty(), "no attempts after the last");
    assert_eq!(notify::count_kind(&pool, "payout_failed").await.unwrap(), 1, "alert raised");
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(0), "reservation released");
    assert!(f.near.sent().is_empty());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn retry_after_unknown_outcome_resubmits_the_same_transaction(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    f.near.fail_next_sends(vec![ToolError::Retryable("timeout waiting for execution".into())]);
    let s = signer(&pool, &f, &t);
    let first = s.process_due(now()).await.unwrap();
    assert!(matches!(first[0].1, SendResult::Failed { .. }));
    let (hash1, tx1): (String, String) = sqlx::query_as("SELECT tx_hash, signed_tx FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();

    let SendResult::Sent { tx_hash } = s.process_due(now() + Duration::seconds(31)).await.unwrap()[0].1.clone() else { panic!() };
    assert_eq!(tx_hash, hash1, "same transaction resubmitted, not a new transfer");
    let (tx2,): (String,) = sqlx::query_as("SELECT signed_tx FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(tx1, tx2);
    assert_eq!(f.near.sent().len(), 1);
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn kill_switch_stops_the_signer(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;
    sqlx::query("UPDATE axom.system_controls SET kill_switch = true").execute(&pool).await.unwrap();
    let SendResult::Rejected { reason } = signer(&pool, &f, &t).send_payout(id, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("kill switch"));
    assert!(f.near.sent().is_empty());
}
