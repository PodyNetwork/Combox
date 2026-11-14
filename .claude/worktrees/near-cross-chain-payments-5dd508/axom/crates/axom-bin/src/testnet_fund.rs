//! `axom testnet-fund [AMOUNT_NEAR] [--to ACCOUNT] [--funder FILE]`: put
//! testnet NEAR in the treasury, for trying payouts out. Testnet only.
//!
//! Faucets such as near-faucet.io only send to accounts that already exist,
//! and a new treasury (an implicit account) only exists after its first
//! deposit. So this sends from a funding account that NEAR's testnet helper
//! creates and funds. With `--funder FILE` that account is kept in FILE
//! (account and key, mode 0600) and reused until it runs low; without it,
//! a new one is made for each transfer and its key is never written down.
//! Either way it is never the treasury key, which stays in OpenBao.
//!
//! The treasury is `--to`, or what the signer at `AXOM_SIGNER_URL` reports.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context};
use axom_core::near::{self, PublicKey, Transaction};
use axom_tools::near_rpc::{JsonRpcNear, NearRpc, TxStatus};
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal::Decimal;
use serde_json::{json, Value};

const HELPER: &str = "https://helper.testnet.near.org/account";
const DEFAULT_NEAR: &str = "2";
const MAX_NEAR: i64 = 5;
/// Left on the funding account for its storage and the transfer's gas.
const RESERVE_YOCTO: u128 = 10_000_000_000_000_000_000_000; // 0.01 NEAR

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// A testnet account this command can send from.
struct Funder {
    account: String,
    key: SigningKey,
}

impl Funder {
    fn public_key(&self) -> PublicKey {
        PublicKey(self.key.verifying_key().to_bytes())
    }

    fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
        let (Some(account), Some(seed)) = (field("ACCOUNT="), field("SIGNING_KEY=")) else {
            bail!("{} isn't a funding account file (ACCOUNT= and SIGNING_KEY=); move it away to start over", path.display());
        };
        let seed: [u8; 32] = hex::decode(&seed).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("bad SIGNING_KEY in {}", path.display()))?;
        Ok(Some(Self { account, key: SigningKey::from_bytes(&seed) }))
    }

    /// Written before the account is funded, so its key is never lost.
    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = format!(
            "# Testnet funding account for ./scripts/fund-testnet. Not the treasury,\n# and worthless off testnet. Delete this file to start a new one.\nACCOUNT={}\nSIGNING_KEY={}\n",
            self.account,
            hex::encode(self.key.to_bytes())
        );
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            o.open(&tmp).and_then(|mut f| f.write_all(text.as_bytes())).with_context(|| format!("writing {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))
    }
}

pub async fn run(args: Vec<String>) -> anyhow::Result<()> {
    let mut amount = DEFAULT_NEAR.to_string();
    let mut to = None;
    let mut funder_file: Option<PathBuf> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => to = Some(it.next().context("--to needs an account")?),
            "--funder" => funder_file = Some(it.next().context("--funder needs a file")?.into()),
            "-h" | "--help" => {
                println!("axom testnet-fund [AMOUNT_NEAR (default {DEFAULT_NEAR}, at most {MAX_NEAR})] [--to ACCOUNT] [--funder FILE]");
                return Ok(());
            }
            _ => amount = a,
        }
    }
    let amount: Decimal = amount.parse().with_context(|| format!("not an amount of NEAR: {amount}"))?;
    ensure!(amount > Decimal::ZERO && amount <= Decimal::from(MAX_NEAR), "send between 0 and {MAX_NEAR} testnet NEAR");
    let want = near::near_to_yocto(amount)?;

    ensure!(env("NEAR_NETWORK").as_deref() != Some("mainnet"), "NEAR_NETWORK=mainnet: this only sends testnet NEAR");
    let rpc_url = env("NEAR_RPC_URL").unwrap_or_else(|| "https://rpc.testnet.near.org".into());
    ensure!(rpc_url.contains("testnet") && !rpc_url.contains("mainnet"), "testnet only (NEAR_RPC_URL={rpc_url})");
    let rpc = JsonRpcNear::new(&rpc_url);

    let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?;
    let to = match to {
        Some(t) => t,
        None => treasury_from_signer(&http).await?,
    };
    near::validate_account_id(&to).map_err(|_| anyhow!("not a NEAR account: {to}"))?;

    // The saved funding account, if it still has enough; otherwise a new one.
    let saved = match &funder_file {
        Some(path) => Funder::load(path)?,
        None => None,
    };
    let mut ready = None;
    if let Some(f) = saved {
        match spendable(&rpc, &f).await? {
            Some((left, access)) if left >= want => {
                println!("Using the saved funding account {} ({} NEAR left).", f.account, near::yocto_to_near_string(left));
                ready = Some((f, left, access));
            }
            Some((left, _)) => println!(
                "The saved funding account {} has {} NEAR left, less than {amount}; making a new one.",
                f.account,
                near::yocto_to_near_string(left)
            ),
            None => println!("The saved funding account {} isn't on NEAR testnet; making a new one.", f.account),
        }
    }
    let (funder, left, access) = match ready {
        Some(r) => r,
        None => {
            let f = Funder {
                account: format!("combox-fund-{}.testnet", hex::encode(rand::random::<[u8; 4]>())),
                key: SigningKey::from_bytes(&rand::random::<[u8; 32]>()),
            };
            if let Some(path) = &funder_file {
                f.save(path)?;
            }
            create_with_helper(&http, &f, &to).await?;
            // Wait until the account and its key are visible.
            let mut found = None;
            for _ in 0..30 {
                if let Ok(Some(s)) = spendable(&rpc, &f).await {
                    if s.0 > 0 {
                        found = Some(s);
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            let (left, access) = found.ok_or_else(|| anyhow!("the funding account didn't appear on NEAR within a minute; try again"))?;
            println!("Made funding account {} ({} NEAR).", f.account, near::yocto_to_near_string(left));
            (f, left, access)
        }
    };

    // Send what was asked, or what a new account got if that's less.
    let deposit = want.min(left);
    ensure!(deposit > 0, "the funding account has nothing to send");
    if deposit < want {
        println!("The helper gave less than {amount} NEAR; sending {} NEAR.", near::yocto_to_near_string(deposit));
    }
    let tx = Transaction::transfer(funder.account.clone(), funder.public_key(), access.nonce + 1, to.clone(), access.block_hash, deposit);
    let sig = funder.key.sign(&tx.signing_hash()).to_bytes();
    let out = rpc.send_tx(&tx.signed_borsh(&sig)).await?;
    ensure!(out.status == TxStatus::Success, "the transfer failed: {:?}", out.status);
    println!("Sent {} testnet NEAR to {to}", near::yocto_to_near_string(deposit));
    println!("  https://testnet.nearblocks.io/txns/{}", out.hash);
    if funder_file.is_some() {
        println!("The funding account has about {} NEAR left for next time.", near::yocto_to_near_string(left - deposit));
    }
    if let Ok(Some(b)) = rpc.view_account(&to).await {
        println!("Treasury balance: {} NEAR", near::yocto_to_near_string(b.amount_yocto));
    }
    Ok(())
}

/// What the account can send (its available balance less the reserve) and
/// its access key, or None if the account or the key isn't on chain.
async fn spendable(rpc: &JsonRpcNear, f: &Funder) -> anyhow::Result<Option<(u128, axom_tools::near_rpc::AccessKeyInfo)>> {
    let Some(b) = rpc.view_account(&f.account).await? else { return Ok(None) };
    let Some(k) = rpc.view_access_key(&f.account, &f.public_key()).await? else { return Ok(None) };
    Ok(Some((b.available_yocto().saturating_sub(RESERVE_YOCTO), k)))
}

/// Has NEAR's testnet helper create and fund the account.
async fn create_with_helper(http: &reqwest::Client, f: &Funder, to: &str) -> anyhow::Result<()> {
    println!("Creating funding account {} with NEAR's testnet helper...", f.account);
    let r = http
        .post(HELPER)
        .json(&json!({ "newAccountId": f.account, "newAccountPublicKey": f.public_key().to_near_string() }))
        .send()
        .await
        .context("couldn't reach NEAR's testnet helper (helper.testnet.near.org)")?;
    if !r.status().is_success() {
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        bail!(
            "NEAR's testnet helper refused ({status}): {}\nInstead, send testnet NEAR to {to} from a testnet wallet.",
            body.chars().take(200).collect::<String>()
        );
    }
    Ok(())
}

/// The treasury account from the signer (`GET /treasury`), on testnet only.
async fn treasury_from_signer(http: &reqwest::Client) -> anyhow::Result<String> {
    let base = env("AXOM_SIGNER_URL").ok_or_else(|| anyhow!("no treasury: pass --to ACCOUNT, or run this where AXOM_SIGNER_URL points at the signer"))?;
    let v: Value = http
        .get(format!("{}/treasury", base.trim_end_matches('/')))
        .send()
        .await
        .with_context(|| format!("couldn't reach the payouts signer at {base}"))?
        .json()
        .await
        .context("the payouts signer gave an unexpected answer")?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        bail!("the payouts signer can't read the treasury key ({e}); is OpenBao unsealed?");
    }
    let network = v.get("network").and_then(Value::as_str).unwrap_or_default();
    ensure!(network == "testnet", "the treasury is on NEAR {network}, not testnet");
    v.get("account").and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("the payouts signer returned no account"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funding_account_file_round_trips_privately() {
        let dir = std::env::temp_dir().join(format!("axom-funder-{}", hex::encode(rand::random::<[u8; 4]>())));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("testnet_funder");
        assert!(Funder::load(&path).unwrap().is_none());

        let f = Funder { account: "combox-fund-abcd1234.testnet".into(), key: SigningKey::from_bytes(&[7u8; 32]) };
        f.save(&path).unwrap();
        let back = Funder::load(&path).unwrap().unwrap();
        assert_eq!(back.account, f.account);
        assert_eq!(back.public_key(), f.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        std::fs::write(&path, "nonsense\n").unwrap();
        assert!(Funder::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
