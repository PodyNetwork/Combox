//! Requires DATABASE_URL. Cross-chain payouts through NEAR Intents: the
//! worker names a destination with their verified NEAR key, payroll carries
//! it into the signed authorization, and the signer deposits wNEAR into a
//! checked 1Click quote (simulated here) and finalizes on delivery.

mod common;

use axom_core::intents::{self, Destination};
use axom_core::near::{usd_to_yocto, Action};
use axom_core::state::PayoutStatus;
use axom_core::Uuid;
use axom_hr::approvals::{self, AdminSession};
use axom_hr::destinations::{self, DestinationErr};
use axom_hr::{cap, notify, payouts};
use axom_signer::{SendResult, Signer};
use axom_tools::intents::{ExecutionStatus, SimulatedIntents};
use common::*;
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal_macros::dec;
use sqlx::PgPool;

const USDC_BASE: &str = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const ADDR: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

fn usdc_on_base() -> Destination {
    Destination { chain: "base".into(), asset_id: USDC_BASE.into(), address: ADDR.into() }
}

fn signer<'a>(pool: &'a PgPool, f: &'a Fixture, t: &'a LocalTransit, sim: &'a SimulatedIntents) -> Signer<'a> {
    Signer { pool, policy: &f.policy, cfg: &f.cfg, transit: t, near: &f.near, notify: &f.notify, intents: Some(sim) }
}

/// The worker signs the destination with their verified NEAR key.
async fn set_destination(pool: &PgPool, f: &Fixture, w: Uuid, sk: &SigningKey, d: &Destination) -> Uuid {
    let ch = destinations::issue_challenge(pool, w, "testnet", d, now()).await.unwrap();
    let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
    destinations::verify_challenge(pool, &f.near, ch.id, &sig, now()).await.unwrap()
}

async fn status(pool: &PgPool, id: Uuid) -> PayoutStatus {
    payouts::status(pool, id).await.unwrap()
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn cross_chain_payout_is_deposited_into_a_checked_intent_and_finalized_on_delivery(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, account, sk) = f.ready_worker_keyed(&pool, dec!(10), 2).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;
    assert_eq!(status(&pool, id).await, PayoutStatus::Approved);

    // The payout keeps the worker's NEAR identity as recipient and carries the destination.
    let (recipient, chain, asset, address, yocto): (String, String, String, String, String) =
        sqlx::query_as("SELECT recipient, dest_chain, dest_asset, dest_address, amount_yocto::text FROM axom.payouts WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((recipient.as_str(), chain.as_str(), asset.as_str(), address.as_str()), (account.as_str(), "base", USDC_BASE, ADDR));
    // 20 USD owed + 1% route fee, at 4 USD/NEAR.
    let expected = usd_to_yocto(dec!(20.2), dec!(4)).unwrap();
    assert_eq!(yocto, expected.to_string());

    let sim = SimulatedIntents::new();
    let s = signer(&pool, &f, &t, &sim);
    let out = s.process_due(now()).await.unwrap();
    let SendResult::Sent { tx_hash } = &out[0].1 else { panic!("{out:?}") };
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);

    // The quote asked for exactly the authorized route.
    let q = &sim.quotes()[0];
    assert_eq!((q.origin_asset.as_str(), q.destination_asset.as_str(), q.recipient.as_str()), ("nep141:wrap.testnet", USDC_BASE, ADDR));
    assert_eq!(q.amount, expected.to_string());
    assert_eq!(q.refund_to, t.public().implicit_account());
    assert_eq!((q.swap_type.as_str(), q.deposit_type.as_str(), q.refund_type.as_str()), ("EXACT_INPUT", "ORIGIN_CHAIN", "ORIGIN_CHAIN"));

    // One NEAR transaction to wrap.testnet: register, wrap, transfer to the deposit address.
    let (dep,): (String,) = sqlx::query_as("SELECT intent_deposit_address FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    let sent = f.near.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].receiver_id, "wrap.testnet");
    assert_eq!(sent[0].actions, intents::deposit_actions(&dep, expected, &format!("axom payout {id}")));
    assert!(matches!(&sent[0].actions[3], Action::FunctionCall { method_name, deposit: 1, .. } if method_name == "ft_transfer"));
    assert_eq!(sim.submitted(), vec![(tx_hash.clone(), dep.clone())], "deposit reported to 1Click");

    // NEAR-final is not enough: finalized only once the intent reports delivery.
    sim.set_status(Some(ExecutionStatus::Processing));
    assert!(s.finalize_sent(now()).await.unwrap().is_empty());
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
    sim.set_status(None);
    assert_eq!(s.finalize_sent(now()).await.unwrap(), vec![id]);
    assert_eq!(status(&pool, id).await, PayoutStatus::Finalized);
    let (st, hashes): (String, serde_json::Value) =
        sqlx::query_as("SELECT intent_status, intent_tx_hashes FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "SUCCESS");
    assert!(hashes[0]["hash"].as_str().unwrap().starts_with("simulated-"));

    // Cap counts the payout, the route fee, and the network fee.
    let u = cap::usage(&pool, cap::month_of(now())).await.unwrap();
    assert!(u.committed_usd > dec!(20.2), "{}", u.committed_usd);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn large_cross_chain_payout_carries_the_destination_through_human_approval(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, sk) = f.ready_worker_keyed(&pool, dec!(30), 2).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;
    assert_eq!(status(&pool, id).await, PayoutStatus::PendingApproval);

    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'o@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let session = AdminSession { user_id: admin, email: "o@example.com".into(), session_id: "s".into(), mfa_verified: true };
    approvals::approve_payout(&pool, &f.policy, &f.approver, &session, id, now()).await.unwrap();
    let (payload,): (serde_json::Value,) = sqlx::query_as("SELECT payload FROM axom.payout_authorizations WHERE payout_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(payload["destination"]["address"], ADDR, "the admin approved the destination, not just the amount");

    let sim = SimulatedIntents::new();
    let out = signer(&pool, &f, &t, &sim).process_due(now()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    assert_eq!(f.near.sent()[0].receiver_id, "wrap.testnet");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn quote_for_a_different_recipient_is_refused_and_nothing_is_signed(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, sk) = f.ready_worker_keyed(&pool, dec!(10), 1).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;
    let sim = SimulatedIntents::new();
    sim.tamper_next_recipient("0x000000000000000000000000000000000000dEaD");
    let out = signer(&pool, &f, &t, &sim).process_due(now()).await.unwrap();
    let SendResult::Failed { reason, final_attempt, .. } = &out[0].1 else { panic!("{out:?}") };
    assert!(reason.contains("recipient"), "{reason}");
    assert!(final_attempt, "a quote that contradicts the authorization is not retried");
    assert!(f.near.sent().is_empty());
    assert_eq!(status(&pool, id).await, PayoutStatus::Failed);
    assert_eq!(cap::usage(&pool, cap::month_of(now())).await.unwrap().used_usd(), dec!(0), "reservation released");
    assert_eq!(notify::count_kind(&pool, "payout_failed").await.unwrap(), 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn quote_below_the_shortfall_floor_is_retried_later(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, sk) = f.ready_worker_keyed(&pool, dec!(10), 1).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;
    let sim = SimulatedIntents::new();
    // Owed 10 USD; max shortfall 3% -> floor 9.70.
    sim.set_out_usd(Some(dec!(9.5)));
    let s = signer(&pool, &f, &t, &sim);
    let out = s.process_due(now()).await.unwrap();
    let SendResult::Failed { reason, final_attempt, next_attempt_at, .. } = out[0].1.clone() else { panic!("{out:?}") };
    assert!(reason.contains("floor"), "{reason}");
    assert!(!final_attempt);
    assert!(f.near.sent().is_empty());
    // Price recovers: the retry goes through.
    sim.set_out_usd(None);
    let out = s.process_due(next_attempt_at.unwrap()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    assert_eq!(status(&pool, id).await, PayoutStatus::Sent);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn refunded_intent_alerts_the_owner(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, sk) = f.ready_worker_keyed(&pool, dec!(10), 1).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;
    let sim = SimulatedIntents::new();
    let s = signer(&pool, &f, &t, &sim);
    assert!(matches!(s.process_due(now()).await.unwrap()[0].1, SendResult::Sent { .. }));
    sim.set_status(Some(ExecutionStatus::Refunded));
    assert!(s.finalize_sent(now()).await.unwrap().is_empty());
    assert_eq!(status(&pool, id).await, PayoutStatus::Refunded);
    assert_eq!(notify::count_kind(&pool, "payout_refunded").await.unwrap(), 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn signer_refuses_a_revoked_destination_or_a_missing_intents_client(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, sk) = f.ready_worker_keyed(&pool, dec!(10), 1).await;
    set_destination(&pool, &f, w, &sk, &usdc_on_base()).await;
    let id = f.payroll(&pool, w).await;

    // No intents client: refused, nothing signed.
    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &f.near, notify: &f.notify, intents: None };
    let SendResult::Rejected { reason } = s.send_payout(id, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("not enabled"), "{reason}");

    // Worker revokes the destination after the authorization was issued.
    assert!(destinations::revoke(&pool, w, "testnet", now()).await.unwrap());
    let sim = SimulatedIntents::new();
    let SendResult::Rejected { reason } = signer(&pool, &f, &t, &sim).send_payout(id, now()).await.unwrap() else { panic!() };
    assert!(reason.contains("no longer"), "{reason}");
    assert!(f.near.sent().is_empty());
    assert!(sim.quotes().is_empty(), "no quote requested for a refused payout");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn destination_must_be_signed_by_the_verified_near_key(pool: PgPool) {
    let t = LocalTransit::new();
    let f = Fixture::new(&pool, t.public()).await;
    let (w, _, _sk) = f.ready_worker_keyed(&pool, dec!(10), 1).await;
    let ch = destinations::issue_challenge(&pool, w, "testnet", &usdc_on_base(), now()).await.unwrap();
    // Someone else's key.
    let other = SigningKey::generate(&mut rand::rngs::OsRng);
    let sig = hex::encode(other.sign(ch.message.as_bytes()).to_bytes());
    assert!(matches!(destinations::verify_challenge(&pool, &f.near, ch.id, &sig, now()).await, Err(DestinationErr::BadSignature)));
    assert!(destinations::active(&pool, w, "testnet").await.unwrap().is_none());

    // Malformed destinations never get a challenge.
    let bad = Destination { chain: "base".into(), asset_id: USDC_BASE.into(), address: "not-an-address".into() };
    assert!(matches!(destinations::issue_challenge(&pool, w, "testnet", &bad, now()).await, Err(DestinationErr::Invalid(_))));

    // A worker without a verified NEAR address cannot name a destination.
    let (w2, _, _) = f.ready_worker_keyed(&pool, dec!(1), 0).await;
    sqlx::query("UPDATE axom.worker_addresses SET revoked_at = now() WHERE worker_id = $1").bind(w2).execute(&pool).await.unwrap();
    assert!(matches!(destinations::issue_challenge(&pool, w2, "testnet", &usdc_on_base(), now()).await, Err(DestinationErr::NoVerifiedAddress)));

    // Without a destination the worker is paid in NEAR as before.
    let id = f.payroll(&pool, w).await;
    let (dest,): (Option<String>,) = sqlx::query_as("SELECT dest_chain FROM axom.payouts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(dest.is_none());
}
