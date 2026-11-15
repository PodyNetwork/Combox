//! Print a treasury's Rhea position from live NEAR RPC (read-only).
//!   cargo run -p axom-tools --example rhea_position -- mainnet <account>
use axom_tools::near_rpc::JsonRpcNear;
use axom_tools::rhea::TreasuryViews;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (network, account) = (a.first().map(String::as_str).unwrap_or("mainnet"), a.get(1).map(String::as_str).unwrap_or("wrap.near"));
    let rpc = JsonRpcNear::new(format!("https://rpc.{network}.fastnear.com"));
    println!("{:#?}", rpc.position(network, account).await.map_err(|e| anyhow::anyhow!("{e}"))?);
    Ok(())
}
