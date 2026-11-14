//! Cross-chain payouts through NEAR Intents.
//!
//! A worker may be paid on another chain (USDC on Base, BTC, SOL, ...)
//! instead of in NEAR. The treasury stays on NEAR: for each such payout the
//! signer asks the 1Click API for a quote (origin asset wNEAR, destination
//! asset and address chosen by the worker), then sends one NEAR transaction
//! to the wNEAR contract that wraps the authorized amount and transfers it to
//! the quote's deposit address. Solvers in the NEAR Intents network deliver
//! the destination asset; if they cannot, the deposit is refunded to the
//! treasury.
//!
//! This module is pure: destination validation, the exact action template
//! the signer may sign, and a checker for that template.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::near::{self, Action, Transaction};

/// 0.00125 NEAR: the NEP-145 storage registration minimum on the wNEAR contract.
pub const WNEAR_STORAGE_DEPOSIT_YOCTO: u128 = 1_250_000_000_000_000_000_000;
/// Gas per function call in the deposit transaction (15 Tgas; unused gas is refunded).
pub const DEPOSIT_CALL_GAS: u64 = 15_000_000_000_000;

/// `testnet` unless `NEAR_NETWORK=mainnet` exactly.
pub fn network_from_env() -> String {
    match std::env::var("NEAR_NETWORK").as_deref() {
        Ok("mainnet") => "mainnet".into(),
        _ => "testnet".into(),
    }
}

/// The wNEAR (NEP-141) contract on a network.
pub fn wnear_contract(network: &str) -> &'static str {
    if network == "mainnet" {
        "wrap.near"
    } else {
        "wrap.testnet"
    }
}

/// The 1Click asset id of wNEAR, the origin asset of every intent payout.
pub fn origin_asset(network: &str) -> String {
    format!("nep141:{}", wnear_contract(network))
}

/// Where a worker wants to be paid when it is not in NEAR on NEAR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    /// 1Click `blockchain` id: `eth`, `base`, `arb`, `sol`, `btc`, `near`, ...
    pub chain: String,
    /// 1Click destination `assetId`, e.g. `nep141:base-0x8335...omft.near`.
    pub asset_id: String,
    /// Recipient address on `chain`.
    pub address: String,
}

/// Address families Axom knows how to validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Evm,
    Solana,
    Bitcoin,
    Near,
    Tron,
}

/// Destination chains Axom accepts, by 1Click `blockchain` id.
pub const SUPPORTED_CHAINS: &[(&str, &str)] = &[
    ("eth", "Ethereum"),
    ("base", "Base"),
    ("arb", "Arbitrum"),
    ("op", "Optimism"),
    ("pol", "Polygon"),
    ("bsc", "BNB Chain"),
    ("avax", "Avalanche"),
    ("gnosis", "Gnosis"),
    ("bera", "Berachain"),
    ("sol", "Solana"),
    ("btc", "Bitcoin"),
    ("tron", "Tron"),
    ("near", "NEAR"),
];

fn family(chain: &str) -> Option<Family> {
    Some(match chain {
        "eth" | "base" | "arb" | "op" | "pol" | "bsc" | "avax" | "gnosis" | "bera" => Family::Evm,
        "sol" => Family::Solana,
        "btc" => Family::Bitcoin,
        "tron" => Family::Tron,
        "near" => Family::Near,
        _ => return None,
    })
}

pub fn chain_name(chain: &str) -> &str {
    SUPPORTED_CHAINS.iter().find(|(id, _)| *id == chain).map(|(_, n)| *n).unwrap_or(chain)
}

const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn base58_len(s: &str) -> Option<usize> {
    if !s.chars().all(|c| BASE58.contains(c)) {
        return None;
    }
    bs58::decode(s).into_vec().ok().map(|v| v.len())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DestinationError {
    #[error("unsupported destination chain {0:?}")]
    UnsupportedChain(String),
    #[error("invalid asset id {0:?} (expected a 1Click asset id: nep141:..., nep245:..., or 1cs_v1:...)")]
    BadAsset(String),
    #[error("invalid {chain} address {address:?}")]
    BadAddress { chain: String, address: String },
}

/// Validate a destination's chain, asset id, and address format.
///
/// The address format check catches typos and wrong-chain addresses; it does
/// not prove the worker controls the address. That is the worker's own
/// statement, signed with their verified NEAR key.
pub fn validate_destination(d: &Destination) -> Result<(), DestinationError> {
    let fam = family(&d.chain).ok_or_else(|| DestinationError::UnsupportedChain(d.chain.clone()))?;
    let asset_ok = ["nep141:", "nep245:", "1cs_v1:"].iter().any(|p| d.asset_id.starts_with(p))
        && d.asset_id.len() <= 200
        && d.asset_id.len() > 7
        && d.asset_id.bytes().all(|b| b.is_ascii_alphanumeric() || b":-_.".contains(&b));
    if !asset_ok {
        return Err(DestinationError::BadAsset(d.asset_id.clone()));
    }
    let a = d.address.as_str();
    let ok = match fam {
        Family::Evm => a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_hexdigit()),
        Family::Solana => base58_len(a) == Some(32),
        Family::Tron => a.starts_with('T') && base58_len(a) == Some(25),
        Family::Bitcoin => {
            let lower = a.to_ascii_lowercase();
            let bech32 = lower.starts_with("bc1")
                && (14..=74).contains(&a.len())
                && (a == lower || a == a.to_ascii_uppercase())
                && lower[3..].chars().all(|c| "qpzry9x8gf2tvdw0s3jn54khce6mua7l".contains(c));
            let legacy = (a.starts_with('1') || a.starts_with('3')) && base58_len(a) == Some(25);
            bech32 || legacy
        }
        Family::Near => near::validate_account_id(a).is_ok(),
    };
    if ok {
        Ok(())
    } else {
        Err(DestinationError::BadAddress { chain: d.chain.clone(), address: d.address.clone() })
    }
}

/// The message a worker signs with their verified NEAR key to name a
/// cross-chain payout destination.
pub fn destination_message(network: &str, worker_id: uuid::Uuid, near_account: &str, d: &Destination, nonce: &str, expires: &str) -> String {
    format!(
        "Axom cross-chain payout destination\nnetwork: {network}\nworker: {worker_id}\nnear account: {near_account}\nchain: {}\nasset: {}\naddress: {}\nnonce: {nonce}\nexpires: {expires}",
        d.chain, d.asset_id, d.address
    )
}

/// The actions of a deposit into a NEAR Intents quote, all on the wNEAR
/// contract, in one atomic receipt:
///
/// 1. `storage_deposit` for the treasury (no-op refund if registered), so
///    `near_deposit` never deducts a registration fee from the amount;
/// 2. `storage_deposit` for the deposit address (same);
/// 3. `near_deposit` of exactly `amount_yocto` (wraps NEAR into wNEAR);
/// 4. `ft_transfer` of exactly `amount_yocto` wNEAR to the deposit address.
pub fn deposit_actions(deposit_address: &str, amount_yocto: u128, memo: &str) -> Vec<Action> {
    let call = |method: &str, args: serde_json::Value, deposit: u128| Action::FunctionCall {
        method_name: method.to_string(),
        args: serde_json::to_vec(&args).expect("json args"),
        gas: DEPOSIT_CALL_GAS,
        deposit,
    };
    vec![
        call("storage_deposit", json!({ "registration_only": true }), WNEAR_STORAGE_DEPOSIT_YOCTO),
        call("storage_deposit", json!({ "account_id": deposit_address, "registration_only": true }), WNEAR_STORAGE_DEPOSIT_YOCTO),
        call("near_deposit", json!({}), amount_yocto),
        call("ft_transfer", json!({ "receiver_id": deposit_address, "amount": amount_yocto.to_string(), "memo": memo }), 1),
    ]
}

/// Check that `tx` is exactly the deposit template for this payout: to the
/// wNEAR contract of `network`, wrapping and transferring `amount_yocto` to
/// `deposit_address`, and nothing else.
pub fn check_deposit_tx(tx: &Transaction, network: &str, deposit_address: &str, amount_yocto: u128, memo: &str) -> Result<(), String> {
    if tx.receiver_id != wnear_contract(network) {
        return Err(format!("deposit goes to {}, not {}", tx.receiver_id, wnear_contract(network)));
    }
    if tx.actions != deposit_actions(deposit_address, amount_yocto, memo) {
        return Err("deposit actions do not match the template".into());
    }
    Ok(())
}

/// Treasury outflow of a deposit beyond the payout amount (two storage
/// registrations and one yoctoNEAR), before gas. Partly refunded when the
/// accounts are already registered.
pub const DEPOSIT_OVERHEAD_YOCTO: u128 = 2 * WNEAR_STORAGE_DEPOSIT_YOCTO + 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::near::{decode_signed_transaction, PublicKey};

    fn d(chain: &str, asset: &str, addr: &str) -> Destination {
        Destination { chain: chain.into(), asset_id: asset.into(), address: addr.into() }
    }

    #[test]
    fn destination_validation() {
        let usdc_base = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
        assert!(validate_destination(&d("base", usdc_base, "0x52908400098527886E0F7030069857D2E4169EE7")).is_ok());
        assert!(validate_destination(&d("base", usdc_base, "0x5290840009852788")).is_err(), "short EVM address");
        assert!(validate_destination(&d("base", usdc_base, "52908400098527886E0F7030069857D2E4169EE7aa")).is_err());
        assert!(validate_destination(&d("dogecoin", usdc_base, "0x52908400098527886E0F7030069857D2E4169EE7")).is_err());
        assert!(validate_destination(&d("base", "base-usdc", "0x52908400098527886E0F7030069857D2E4169EE7")).is_err());
        assert!(validate_destination(&d("base", "1cs_v1:base:erc20:0x0382e3fee4a420bd446367d468a6f00225853420", "0x52908400098527886E0F7030069857D2E4169EE7")).is_ok());
        assert!(validate_destination(&d("sol", "nep141:sol.omft.near", "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM")).is_ok());
        assert!(validate_destination(&d("sol", "nep141:sol.omft.near", "0x52908400098527886E0F7030069857D2E4169EE7")).is_err());
        assert!(validate_destination(&d("btc", "nep141:btc.omft.near", "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")).is_ok());
        assert!(validate_destination(&d("btc", "nep141:btc.omft.near", "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2")).is_ok());
        assert!(validate_destination(&d("btc", "nep141:btc.omft.near", "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdp")).is_ok(), "format only, no checksum");
        assert!(validate_destination(&d("btc", "nep141:btc.omft.near", "bc1qAR0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")).is_err(), "mixed case bech32");
        assert!(validate_destination(&d("near", "nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1", "alice.near")).is_ok());
        assert!(validate_destination(&d("near", "nep141:wrap.near", "Alice.near")).is_err());
        assert!(validate_destination(&d("tron", "nep141:tron.omft.near", "TJRabPrwbZy45sbavfcjinPJC18kjpRTv8")).is_ok());
    }

    #[test]
    fn deposit_template_round_trips_and_is_strict() {
        let pk = PublicKey([3u8; 32]);
        let dep = "a".repeat(64);
        let tx = Transaction {
            signer_id: pk.implicit_account(),
            public_key: pk,
            nonce: 7,
            receiver_id: "wrap.testnet".into(),
            block_hash: [1u8; 32],
            actions: deposit_actions(&dep, 5_000, "axom payout x"),
        };
        let (decoded, _) = decode_signed_transaction(&tx.signed_borsh(&[0u8; 64])).unwrap();
        assert_eq!(decoded, tx);
        assert!(check_deposit_tx(&decoded, "testnet", &dep, 5_000, "axom payout x").is_ok());
        assert!(check_deposit_tx(&decoded, "mainnet", &dep, 5_000, "axom payout x").is_err(), "wrong wNEAR contract");
        assert!(check_deposit_tx(&decoded, "testnet", &dep, 5_001, "axom payout x").is_err(), "amount differs");
        assert!(check_deposit_tx(&decoded, "testnet", &"b".repeat(64), 5_000, "axom payout x").is_err(), "deposit address differs");
        assert_eq!(tx.total_deposit(), 5_000 + DEPOSIT_OVERHEAD_YOCTO);
        // ft_transfer carries the amount as a JSON string and exactly one yoctoNEAR.
        let Action::FunctionCall { method_name, args, deposit, .. } = &tx.actions[3] else { panic!() };
        assert_eq!(method_name, "ft_transfer");
        assert_eq!(*deposit, 1);
        let v: serde_json::Value = serde_json::from_slice(args).unwrap();
        assert_eq!(v["amount"], "5000");
        assert_eq!(v["receiver_id"], dep);
    }

    /// Golden vector produced independently with near-js
    /// (`@near-js/transactions` 1.3.3 `encodeTransaction`, same inputs), so
    /// the FunctionCall encoder and the JSON args are checked against NEAR's
    /// own JavaScript implementation.
    #[test]
    fn deposit_tx_matches_near_js() {
        let pk = PublicKey([3u8; 32]);
        let tx = Transaction {
            signer_id: pk.implicit_account(),
            public_key: pk,
            nonce: 7,
            receiver_id: "wrap.testnet".into(),
            block_hash: [1u8; 32],
            actions: deposit_actions(&"a".repeat(64), 5_000, "axom payout x"),
        };
        assert_eq!(hex::encode(tx.to_borsh()), "400000003033303330333033303330333033303330333033303330333033303330333033303330333033303330333033303330333033303330333033303330333033303300030303030303030303030303030303030303030303030303030303030303030307000000000000000c000000777261702e746573746e6574010101010101010101010101010101010101010101010101010101010101010104000000020f00000073746f726167655f6465706f7369741a0000007b22726567697374726174696f6e5f6f6e6c79223a747275657d00f0ab75a40d00000000485637193cc34300000000000000020f00000073746f726167655f6465706f7369746a0000007b226163636f756e745f6964223a2261616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161222c22726567697374726174696f6e5f6f6e6c79223a747275657d00f0ab75a40d00000000485637193cc34300000000000000020c0000006e6561725f6465706f736974020000007b7d00f0ab75a40d000088130000000000000000000000000000020b00000066745f7472616e73666572790000007b22616d6f756e74223a2235303030222c226d656d6f223a2261786f6d207061796f75742078222c2272656365697665725f6964223a2261616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161616161227d00f0ab75a40d000001000000000000000000000000000000");
    }

    #[test]
    fn function_call_borsh_layout() {
        // Hand-assembled from NEAR's schema: Action::FunctionCall = tag 2,
        // method_name: String, args: Vec<u8>, gas: u64, deposit: u128.
        let pk = PublicKey([0u8; 32]);
        let tx = Transaction {
            signer_id: "a.testnet".into(),
            public_key: pk,
            nonce: 1,
            receiver_id: "wrap.testnet".into(),
            block_hash: [0u8; 32],
            actions: vec![Action::FunctionCall { method_name: "near_deposit".into(), args: b"{}".to_vec(), gas: 15_000_000_000_000, deposit: 1 }],
        };
        let mut want = Vec::new();
        want.extend_from_slice(&9u32.to_le_bytes());
        want.extend_from_slice(b"a.testnet");
        want.push(0);
        want.extend_from_slice(&[0u8; 32]);
        want.extend_from_slice(&1u64.to_le_bytes());
        want.extend_from_slice(&12u32.to_le_bytes());
        want.extend_from_slice(b"wrap.testnet");
        want.extend_from_slice(&[0u8; 32]);
        want.extend_from_slice(&1u32.to_le_bytes());
        want.push(2);
        want.extend_from_slice(&12u32.to_le_bytes());
        want.extend_from_slice(b"near_deposit");
        want.extend_from_slice(&2u32.to_le_bytes());
        want.extend_from_slice(b"{}");
        want.extend_from_slice(&15_000_000_000_000u64.to_le_bytes());
        want.extend_from_slice(&1u128.to_le_bytes());
        assert_eq!(tx.to_borsh(), want);
    }
}
