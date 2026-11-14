//! Live Rhea lending on NEAR testnet (opt-in, `--ignored`). Runs the real
//! treasury planner and signer code against real testnet RPC and Rhea's
//! testnet lending contract, signing with a throwaway local key (stands in
//! for OpenBao; the OpenBao path is covered by `openbao_real`).
//!
//!   RHEA_LIVE_SEED_FILE=/path/seed cargo test -p axom-signer --test rhea_live -- --ignored --nocapture
//!
//! The seed file holds 64 hex chars; it is created if missing. Fund the
//! printed implicit account with ~2 testnet NEAR from a testnet wallet, then
//! run again. The test makes four real moves: register, supply, withdraw,
//! unwrap. `RHEA_LIVE_RETURN_TO=<account>` sends what's left there at the end.

mod common;

use axom_core::near::{yocto_to_near_string, Transaction, YOCTO_PER_NEAR as N};
use axom_hr::{PolicyConfig, PriceQuote};
use axom_signer::treasury::TreasuryResult;
use axom_signer::Signer;
use axom_tools::near_rpc::{testnet_rpc_url, JsonRpcNear, NearRpc, TxStatus};
use axom_tools::rhea::TreasuryViews;
use common::*;
use ed25519_dalek::Signer as _;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn live_policy() -> PolicyConfig {
    let mut file = policy().file;
    file.treasury.yield_enabled = true;
    file.treasury.min_liquid_near = dec!(0.2);
    file.treasury.min_move_near = dec!(0.1);
    PolicyConfig::from_file(file).unwrap()
}

fn price(usd_per_near: Decimal) -> Option<PriceQuote> {
    Some(PriceQuote { usd_per_near, fetched_at: chrono::Utc::now() })
}

fn link(hash: &str) -> String {
    format!("https://testnet.nearblocks.io/txns/{hash}")
}

/// One tick; panics unless it made the expected move and it succeeded. Waits for finality afterwards.
async fn expect_move(s: &Signer<'_>, rpc: &JsonRpcNear, p: Option<PriceQuote>, kind: &str) -> String {
    let r = s.manage_treasury(rpc, p, chrono::Utc::now()).await.unwrap();
    let TreasuryResult::Moved { kind: k, amount_near, tx_hash, status } = r else { panic!("expected {kind}, got {r:?}") };
    println!("{k:>8} {amount_near:>10} NEAR  {status}  {}", link(&tx_hash));
    assert_eq!((k.as_str(), status.as_str()), (kind, "succeeded"));
    // The planner reads with finality "final"; give the chain a few blocks.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    tx_hash
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
#[ignore = "moves real testnet NEAR through Rhea's testnet lending contract"]
async fn rhea_testnet_register_supply_withdraw_unwrap(pool: PgPool) {
    let path = std::env::var("RHEA_LIVE_SEED_FILE").expect("RHEA_LIVE_SEED_FILE");
    let seed: [u8; 32] = match std::fs::read_to_string(&path) {
        Ok(t) => hex::decode(t.trim()).unwrap().try_into().unwrap(),
        Err(_) => {
            let s: [u8; 32] = rand::random();
            std::fs::write(&path, hex::encode(s)).unwrap();
            s
        }
    };
    let t = LocalTransit::from_seed(seed);
    let treasury = t.public().implicit_account();
    let rpc = JsonRpcNear::new(testnet_rpc_url());
    let start = rpc.position("testnet", &treasury).await.unwrap();
    println!("treasury {treasury}: {} NEAR liquid, {} NEAR supplied on Rhea", yocto_to_near_string(start.liquid_yocto), yocto_to_near_string(start.supplied_yocto));
    assert!(start.liquid_yocto + start.supplied_yocto >= 15 * N / 10, "fund {treasury} with ~2 testnet NEAR, then run again");

    let mut f = Fixture::new(&pool, t.public()).await;
    f.policy = live_policy();
    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &rpc, notify: &f.notify, intents: None };

    // At 5000 USD/NEAR the 500 USD cap needs only 0.1 NEAR liquid: most of the treasury is surplus.
    let calm = price(dec!(5000));
    if !start.registered {
        expect_move(&s, &rpc, calm, "register").await;
    }
    expect_move(&s, &rpc, calm, "supply").await;
    let after_supply = rpc.position("testnet", &treasury).await.unwrap();
    println!("supplied on Rhea: {} NEAR at {}% APR", yocto_to_near_string(after_supply.supplied_yocto), after_supply.supply_apr * dec!(100));
    assert!(after_supply.supplied_yocto > start.supplied_yocto, "Rhea credited the supply");

    // At 1 USD/NEAR payroll may need 500 NEAR: the floor is above everything, so it all comes home.
    let tight = price(dec!(1));
    expect_move(&s, &rpc, tight, "withdraw").await;
    let after_withdraw = rpc.position("testnet", &treasury).await.unwrap();
    assert!(after_withdraw.wnear_yocto > 0, "Rhea paid the withdrawal back to the treasury as wNEAR");
    expect_move(&s, &rpc, tight, "unwrap").await;
    let end = rpc.position("testnet", &treasury).await.unwrap();
    println!(
        "back home: {} NEAR liquid, {} NEAR still supplied (interest accrues per block), {} wNEAR",
        yocto_to_near_string(end.liquid_yocto),
        yocto_to_near_string(end.supplied_yocto),
        yocto_to_near_string(end.wnear_yocto)
    );
    assert!(end.wnear_yocto < N / 1000);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.treasury_moves WHERE status = 'succeeded'").fetch_one(&pool).await.unwrap();
    assert!(n >= 3);

    // Optionally send what's left back to whoever funded it.
    if let Ok(to) = std::env::var("RHEA_LIVE_RETURN_TO") {
        let pk = t.public();
        let access = rpc.view_access_key(&treasury, &pk).await.unwrap().unwrap();
        let amount = end.liquid_yocto.saturating_sub(N / 100);
        let tx = Transaction::transfer(treasury.clone(), pk, access.nonce + 1, to.clone(), access.block_hash, amount);
        let sig = t.key.sign(&tx.signing_hash()).to_bytes();
        let out = rpc.send_tx(&tx.signed_borsh(&sig)).await.unwrap();
        assert_eq!(out.status, TxStatus::Success);
        println!("returned {} NEAR to {to}: {}", yocto_to_near_string(amount), link(&out.hash));
    }
}
