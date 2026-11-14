//! Testnet-only stand-in for the owner's cold-wallet top-up of the treasury.
//! Signs a transfer with a local key (the owner's, never ForeMan's) and sends it.
//!
//!   COLD_KEY_FILE=cold.env COLD_ACCOUNT=me.testnet TO=<treasury implicit account> AMOUNT_NEAR=1 \
//!   cargo run -p axom-signer --example testnet_fund
//!
//! COLD_KEY_FILE holds `SIGNING_KEY=<hex seed>` as printed by `axom keygen`.
//! Refuses to run against anything but NEAR testnet.

use axom_core::near::{self, PublicKey, Transaction};
use axom_tools::near_rpc::{testnet_rpc_url, JsonRpcNear, NearRpc, TxStatus};
use ed25519_dalek::{Signer as _, SigningKey};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let rpc = testnet_rpc_url();
    anyhow::ensure!(rpc.contains("testnet") && !rpc.contains("mainnet"), "testnet only (NEAR_RPC_URL={rpc})");
    let key_file = std::env::var("COLD_KEY_FILE")?;
    let seed_hex = std::fs::read_to_string(&key_file)?
        .lines()
        .find_map(|l| l.strip_prefix("SIGNING_KEY=").map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("no SIGNING_KEY= line in {key_file}"))?;
    let seed: [u8; 32] = hex::decode(seed_hex.trim())?.try_into().map_err(|_| anyhow::anyhow!("seed must be 32 bytes"))?;
    let sk = SigningKey::from_bytes(&seed);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let from = std::env::var("COLD_ACCOUNT")?;
    let to = std::env::var("TO")?;
    near::validate_account_id(&to)?;
    let amount: rust_decimal::Decimal = std::env::var("AMOUNT_NEAR")?.parse()?;
    anyhow::ensure!(amount <= rust_decimal::Decimal::from(5), "keep testnet top-ups small (<= 5 NEAR)");

    let near_rpc = JsonRpcNear::new(rpc);
    let access = near_rpc.view_access_key(&from, &pk).await?.ok_or_else(|| anyhow::anyhow!("{pk:?} is not a key on {from}"))?;
    let tx = Transaction::transfer(from.clone(), pk, access.nonce + 1, to.clone(), access.block_hash, near::near_to_yocto(amount)?);
    let sig = sk.sign(&tx.signing_hash()).to_bytes();
    let out = near_rpc.send_tx(&tx.signed_borsh(&sig)).await?;
    anyhow::ensure!(out.status == TxStatus::Success, "transfer failed: {:?}", out.status);
    println!("sent {amount} NEAR {from} -> {to}");
    println!("https://testnet.nearblocks.io/txns/{}", out.hash);
    Ok(())
}
