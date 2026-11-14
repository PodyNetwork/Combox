//! Requires DATABASE_URL. Treasury yield on Rhea lending: the signer
//! registers, supplies the surplus above the payroll floor, withdraws and
//! unwraps when payroll needs it, and stops under the kill switch. Chain
//! state is a mock position; transactions go through the node-like mock.

mod common;

use axom_core::near::{Action, YOCTO_PER_NEAR as N};
use axom_core::rhea::{self, Move};
use axom_hr::{notify, PolicyConfig, PriceQuote};
use axom_signer::treasury::TreasuryResult;
use axom_signer::Signer;
use axom_tools::rhea::MockTreasury;
use axom_tools::ToolError;
use chrono::Duration;
use common::*;
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn yield_policy(on: bool) -> PolicyConfig {
    let mut file = policy().file;
    file.treasury.yield_enabled = on;
    PolicyConfig::from_file(file).unwrap()
}

fn price() -> Option<PriceQuote> {
    Some(PriceQuote { usd_per_near: dec!(4), fetched_at: now() - Duration::minutes(1) })
}

/// Run one tick and apply the move to the mock chain if it succeeded.
async fn tick(s: &Signer<'_>, chain: &MockTreasury, at: chrono::DateTime<chrono::Utc>) -> TreasuryResult {
    let r = s.manage_treasury(chain, price(), at).await.unwrap();
    if let TreasuryResult::Moved { status, .. } = &r {
        if status == "succeeded" {
            let (plan,): (serde_json::Value,) = sqlx::query_as("SELECT plan FROM axom.treasury_moves ORDER BY created_at DESC LIMIT 1").fetch_one(s.pool).await.unwrap();
            let m: Move = serde_json::from_value(plan["action"].clone()).unwrap();
            chain.apply(&m);
        }
    }
    r
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn supplies_the_surplus_and_brings_it_back_when_payroll_needs_it(pool: PgPool) {
    let t = LocalTransit::new();
    let mut f = Fixture::new(&pool, t.public()).await;
    f.policy = yield_policy(true);
    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &f.near, notify: &f.notify, intents: None };
    let chain = MockTreasury::new(1000 * N);
    let treasury = t.public().implicit_account();

    // 1. Register on Rhea lending (0.1 NEAR storage), then supply.
    let r = tick(&s, &chain, now()).await;
    assert!(matches!(&r, TreasuryResult::Moved { kind, status, .. } if kind == "register" && status == "succeeded"), "{r:?}");
    let r = tick(&s, &chain, now()).await;
    let TreasuryResult::Moved { kind, status, .. } = &r else { panic!("{r:?}") };
    assert_eq!((kind.as_str(), status.as_str()), ("supply", "succeeded"));

    // The floor: the whole 500 USD cap is still spendable this month = 125 NEAR at 4 USD/NEAR, + 1 NEAR minimum.
    let (plan,): (serde_json::Value,) = sqlx::query_as("SELECT plan FROM axom.treasury_status").fetch_one(&pool).await.unwrap();
    // Yocto amounts are strings in JSON, so JSONB round trips keep every digit.
    let floor: u128 = plan["floor_yocto"].as_str().unwrap().parse().unwrap();
    let target: u128 = plan["target_yocto"].as_str().unwrap().parse().unwrap();
    assert_eq!(floor, 126 * N);
    assert_eq!(target, 126 * N + 126 * N / 100 * 25);
    let pos = chain.state.lock().unwrap().clone();
    assert!(pos.liquid_yocto >= target, "liquid stays at or above the target after supplying");
    assert!(pos.supplied_yocto > 800 * N);

    // What went on chain: one tx to the lending contract, one to wrap.testnet, both from the treasury, both exact templates.
    let sent = f.near.sent();
    assert_eq!(sent.len(), 2);
    let register = sent.iter().find(|x| x.receiver_id == rhea::lending_contract("testnet")).unwrap();
    rhea::check_tx(register, &Move::Register { storage_yocto: N / 10 }, "testnet", &treasury).unwrap();
    let supply = sent.iter().find(|x| x.receiver_id == "wrap.testnet").unwrap();
    rhea::check_tx(supply, &Move::Supply { amount_yocto: pos.supplied_yocto }, "testnet", &treasury).unwrap();
    assert!(matches!(&supply.actions[2], Action::FunctionCall { method_name, .. } if method_name == "ft_transfer_call"));

    // 2. Nothing more to do.
    assert!(matches!(tick(&s, &chain, now()).await, TreasuryResult::Held { .. }));

    // 3. Payouts drain the liquid balance below the floor: withdraw back to the target, then unwrap.
    chain.state.lock().unwrap().liquid_yocto = 40 * N;
    let r = tick(&s, &chain, now()).await;
    let TreasuryResult::Moved { kind, .. } = &r else { panic!("{r:?}") };
    assert_eq!(kind, "withdraw");
    let r = tick(&s, &chain, now()).await;
    let TreasuryResult::Moved { kind, .. } = &r else { panic!("{r:?}") };
    assert_eq!(kind, "unwrap");
    let pos = chain.state.lock().unwrap().clone();
    assert_eq!(pos.liquid_yocto, target, "back at the target");
    assert_eq!(pos.wnear_yocto, 0);

    // Every move is recorded with its policy version and plan.
    let (n, ok): (i64, i64) = sqlx::query_as("SELECT count(*), count(*) FILTER (WHERE status = 'succeeded') FROM axom.treasury_moves").fetch_one(&pool).await.unwrap();
    assert_eq!((n, ok), (4, 4));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn kill_switch_and_disabled_yield_stop_supplying(pool: PgPool) {
    let t = LocalTransit::new();
    let mut f = Fixture::new(&pool, t.public()).await;
    f.policy = yield_policy(true);
    let chain = MockTreasury::new(1000 * N);
    sqlx::query("UPDATE axom.system_controls SET kill_switch = true").execute(&pool).await.unwrap();
    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &f.near, notify: &f.notify, intents: None };
    assert_eq!(tick(&s, &chain, now()).await, TreasuryResult::Halted);
    sqlx::query("UPDATE axom.system_controls SET kill_switch = false").execute(&pool).await.unwrap();

    // Yield off and never used: nothing is read or sent.
    let off = yield_policy(false);
    let s_off = Signer { policy: &off, ..s };
    assert!(matches!(tick(&s_off, &chain, now()).await, TreasuryResult::Held { .. }));
    assert!(f.near.sent().is_empty());

    // Supplied while on, then switched off: everything comes home.
    let s_on = Signer { policy: &f.policy, ..s_off };
    tick(&s_on, &chain, now()).await; // register
    tick(&s_on, &chain, now()).await; // supply
    let supplied = chain.state.lock().unwrap().supplied_yocto;
    assert!(supplied > 0);
    let s_off = Signer { policy: &off, ..s_on };
    let r = tick(&s_off, &chain, now()).await;
    assert!(matches!(&r, TreasuryResult::Moved { kind, amount_near, .. } if kind == "withdraw" && amount_near == &axom_core::near::yocto_to_near_string(supplied)), "{r:?}");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn unknown_outcome_resubmits_the_same_bytes_and_blocks_new_moves(pool: PgPool) {
    let t = LocalTransit::new();
    let mut f = Fixture::new(&pool, t.public()).await;
    f.policy = yield_policy(true);
    let chain = MockTreasury::new(1000 * N);
    chain.state.lock().unwrap().registered = true;
    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &f.near, notify: &f.notify, intents: None };
    f.near.fail_next_sends(vec![ToolError::Retryable("rpc timeout".into())]);
    let r = s.manage_treasury(&chain, price(), now()).await.unwrap();
    let TreasuryResult::Moved { status, tx_hash, .. } = r else { panic!("{r:?}") };
    assert_eq!(status, "submitted");
    // While it is in flight, nothing new is planned or signed.
    assert_eq!(s.manage_treasury(&chain, price(), now()).await.unwrap(), TreasuryResult::Waiting { tx_hash: tx_hash.clone() });
    assert!(f.near.sent().is_empty());
    // After a minute the exact same bytes go out again and settle.
    s.manage_treasury(&chain, price(), now() + Duration::seconds(61)).await.unwrap();
    assert_eq!(f.near.sent().len(), 1);
    assert_eq!(bs58::encode(f.near.sent()[0].signing_hash()).into_string(), tx_hash);
    let (st,): (String,) = sqlx::query_as("SELECT status FROM axom.treasury_moves").fetch_one(&pool).await.unwrap();
    assert_eq!(st, "succeeded");
    assert_eq!(notify::count_kind(&pool, "treasury_move_failed").await.unwrap(), 0);
}
