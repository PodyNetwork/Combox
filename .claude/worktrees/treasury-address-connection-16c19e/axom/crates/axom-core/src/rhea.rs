//! Rhea Finance lending (formerly Burrow) on NEAR: the treasury's idle NEAR
//! earns supply interest while payroll doesn't need it.
//!
//! The treasury only ever does four things here, each a fixed template:
//! - register its own account on the lending contract (`storage_deposit`);
//! - supply: wrap NEAR and `ft_transfer_call` the wNEAR to the lending
//!   contract with an empty `msg`, which credits the sender's (the
//!   treasury's) supplied balance. Not collateral, never borrowed against;
//! - withdraw: `execute` with one `Withdraw` action, which sends the wNEAR
//!   back to the caller (the treasury);
//! - unwrap: `near_withdraw` on the wNEAR contract.
//!
//! None of these can send value to any account other than the treasury's own
//! position. [`check_tx`] rejects anything that isn't exactly one of them.
//!
//! Verified on mainnet (2026-11-14): `contract.main.burrow.near` reports
//! wNEAR `extra_decimals: 0` (internal amounts equal wNEAR amounts) and a
//! 0.1 NEAR storage minimum; plain-supply deposits arrive as
//! `ft_on_transfer {"msg": ""}`; accounts withdraw wNEAR with
//! `execute {"actions":[{"Withdraw":{"token_id":"wrap.near"}}]}` and 1 yocto,
//! with no oracle call, and the contract pays them with `ft_transfer`.

use serde_json::json;

use crate::intents::{wnear_contract, WNEAR_STORAGE_DEPOSIT_YOCTO};
use crate::near::{Action, Transaction};

/// Rhea's lending contract on a network.
pub fn lending_contract(network: &str) -> &'static str {
    if network == "mainnet" {
        "contract.main.burrow.near"
    } else {
        "contract.1689937928.burrow.testnet"
    }
}

/// Upper bound we accept for the lending contract's storage minimum (it
/// reports 0.1 NEAR; refuse to pay more than 0.25 NEAR if that changes).
pub const MAX_LENDING_STORAGE_YOCTO: u128 = 250_000_000_000_000_000_000_000;
pub const SUPPLY_GAS: u64 = 100_000_000_000_000;
pub const WITHDRAW_GAS: u64 = 150_000_000_000_000;
pub const SMALL_GAS: u64 = 15_000_000_000_000;

/// One treasury move. Every variant keeps value inside the treasury's own
/// accounts (its NEAR account and its supplied balance on Rhea).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Move {
    /// Register the treasury on the lending contract.
    Register {
        #[serde(with = "crate::near::yocto_str")]
        storage_yocto: u128,
    },
    /// Wrap and supply this much NEAR.
    Supply {
        #[serde(with = "crate::near::yocto_str")]
        amount_yocto: u128,
    },
    /// Withdraw this much wNEAR from the supplied balance.
    Withdraw {
        #[serde(with = "crate::near::yocto_str")]
        amount_yocto: u128,
    },
    /// Unwrap this much wNEAR back to NEAR.
    Unwrap {
        #[serde(with = "crate::near::yocto_str")]
        amount_yocto: u128,
    },
}

impl Move {
    pub fn kind(&self) -> &'static str {
        match self {
            Move::Register { .. } => "register",
            Move::Supply { .. } => "supply",
            Move::Withdraw { .. } => "withdraw",
            Move::Unwrap { .. } => "unwrap",
        }
    }

    pub fn amount_yocto(&self) -> u128 {
        match *self {
            Move::Register { storage_yocto } => storage_yocto,
            Move::Supply { amount_yocto } | Move::Withdraw { amount_yocto } | Move::Unwrap { amount_yocto } => amount_yocto,
        }
    }

    /// The contract the transaction goes to.
    pub fn receiver(&self, network: &str) -> &'static str {
        match self {
            Move::Register { .. } | Move::Withdraw { .. } => lending_contract(network),
            Move::Supply { .. } | Move::Unwrap { .. } => wnear_contract(network),
        }
    }

    /// The actions of the transaction, from a fixed template.
    pub fn actions(&self, network: &str) -> Vec<Action> {
        let call = |method: &str, args: serde_json::Value, gas: u64, deposit: u128| Action::FunctionCall {
            method_name: method.to_string(),
            args: serde_json::to_vec(&args).expect("json args"),
            gas,
            deposit,
        };
        let wnear = wnear_contract(network);
        match *self {
            // No account_id: registers the caller, the treasury.
            Move::Register { storage_yocto } => vec![call("storage_deposit", json!({}), SMALL_GAS, storage_yocto)],
            Move::Supply { amount_yocto } => vec![
                // Make sure the treasury holds wNEAR storage, so near_deposit never keeps a fee.
                call("storage_deposit", json!({ "registration_only": true }), SMALL_GAS, WNEAR_STORAGE_DEPOSIT_YOCTO),
                call("near_deposit", json!({}), SMALL_GAS, amount_yocto),
                // Empty msg: credit the sender's supplied balance (not collateral).
                call(
                    "ft_transfer_call",
                    json!({ "receiver_id": lending_contract(network), "amount": amount_yocto.to_string(), "msg": "" }),
                    SUPPLY_GAS,
                    1,
                ),
            ],
            Move::Withdraw { amount_yocto } => vec![call(
                "execute",
                json!({ "actions": [{ "Withdraw": { "token_id": wnear, "max_amount": amount_yocto.to_string() } }] }),
                WITHDRAW_GAS,
                1,
            )],
            Move::Unwrap { amount_yocto } => vec![call("near_withdraw", json!({ "amount": amount_yocto.to_string() }), SMALL_GAS, 1)],
        }
    }

    /// NEAR leaving the treasury's liquid balance with the transaction
    /// (attached deposits), before gas.
    pub fn outflow_yocto(&self, network: &str) -> u128 {
        self.actions(network)
            .iter()
            .map(|a| match a {
                Action::Transfer { deposit } | Action::FunctionCall { deposit, .. } => *deposit,
            })
            .sum()
    }
}

/// Build the transaction for a move.
pub fn transaction(m: &Move, network: &str, signer_id: String, public_key: crate::near::PublicKey, nonce: u64, block_hash: [u8; 32]) -> Transaction {
    Transaction { signer_id, public_key, nonce, receiver_id: m.receiver(network).to_string(), block_hash, actions: m.actions(network) }
}

/// Check that `tx` is exactly the template for `m`, from the treasury, and
/// nothing else.
pub fn check_tx(tx: &Transaction, m: &Move, network: &str, treasury: &str) -> Result<(), String> {
    if tx.signer_id != treasury {
        return Err(format!("signer {} is not the treasury {treasury}", tx.signer_id));
    }
    if tx.receiver_id != m.receiver(network) {
        return Err(format!("{} goes to {}, not {}", m.kind(), tx.receiver_id, m.receiver(network)));
    }
    if tx.actions != m.actions(network) {
        return Err(format!("{} actions do not match the template", m.kind()));
    }
    if let Move::Register { storage_yocto } = m {
        if *storage_yocto > MAX_LENDING_STORAGE_YOCTO {
            return Err(format!("storage deposit {storage_yocto} is above the {MAX_LENDING_STORAGE_YOCTO} bound"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::near::{decode_signed_transaction, PublicKey};

    fn tx(m: &Move) -> Transaction {
        let pk = PublicKey([5u8; 32]);
        transaction(m, "mainnet", pk.implicit_account(), pk, 9, [2u8; 32])
    }

    #[test]
    fn templates_never_pay_anyone_else() {
        let treasury = PublicKey([5u8; 32]).implicit_account();
        for m in [
            Move::Register { storage_yocto: 100_000_000_000_000_000_000_000 },
            Move::Supply { amount_yocto: 7 * 10u128.pow(24) },
            Move::Withdraw { amount_yocto: 3 * 10u128.pow(24) },
            Move::Unwrap { amount_yocto: 3 * 10u128.pow(24) },
        ] {
            let t = tx(&m);
            let (decoded, _) = decode_signed_transaction(&t.signed_borsh(&[0u8; 64])).unwrap();
            check_tx(&decoded, &m, "mainnet", &treasury).unwrap();
            for a in &t.actions {
                let Action::FunctionCall { method_name, args, .. } = a else { panic!("only function calls") };
                let v: serde_json::Value = serde_json::from_slice(args).unwrap();
                // The only receiver ever named in arguments is the lending contract.
                if let Some(r) = v.get("receiver_id") {
                    assert_eq!(r, "contract.main.burrow.near", "{method_name}");
                }
                assert!(v.get("account_id").is_none(), "{method_name} must not register or pay a third party");
                let s = String::from_utf8_lossy(args);
                for forbidden in ["Borrow", "IncreaseCollateral", "account_id"] {
                    assert!(!s.contains(forbidden), "{method_name}: {s}");
                }
            }
        }
    }

    #[test]
    fn supply_and_withdraw_shapes() {
        let s = tx(&Move::Supply { amount_yocto: 5 });
        assert_eq!(s.receiver_id, "wrap.near");
        let Action::FunctionCall { method_name, args, deposit, gas } = &s.actions[2] else { panic!() };
        assert_eq!((method_name.as_str(), *deposit, *gas), ("ft_transfer_call", 1, SUPPLY_GAS));
        let v: serde_json::Value = serde_json::from_slice(args).unwrap();
        assert_eq!(v, json!({ "receiver_id": "contract.main.burrow.near", "amount": "5", "msg": "" }));
        assert_eq!(Move::Supply { amount_yocto: 5 }.outflow_yocto("mainnet"), 5 + 1 + WNEAR_STORAGE_DEPOSIT_YOCTO);

        let w = tx(&Move::Withdraw { amount_yocto: 9 });
        assert_eq!(w.receiver_id, "contract.main.burrow.near");
        let Action::FunctionCall { method_name, args, deposit, .. } = &w.actions[0] else { panic!() };
        assert_eq!((method_name.as_str(), *deposit), ("execute", 1));
        let v: serde_json::Value = serde_json::from_slice(args).unwrap();
        assert_eq!(v, json!({ "actions": [{ "Withdraw": { "token_id": "wrap.near", "max_amount": "9" } }] }));
        assert_eq!(Move::Withdraw { amount_yocto: 9 }.outflow_yocto("mainnet"), 1, "withdraw costs one yocto, nothing leaves");
    }

    #[test]
    fn checker_rejects_anything_else() {
        let treasury = PublicKey([5u8; 32]).implicit_account();
        let m = Move::Supply { amount_yocto: 5 };
        let mut t = tx(&m);
        t.actions[2] = Action::FunctionCall {
            method_name: "ft_transfer_call".into(),
            args: serde_json::to_vec(&json!({ "receiver_id": "attacker.near", "amount": "5", "msg": "" })).unwrap(),
            gas: SUPPLY_GAS,
            deposit: 1,
        };
        assert!(check_tx(&t, &m, "mainnet", &treasury).is_err());
        assert!(check_tx(&tx(&m), &Move::Supply { amount_yocto: 6 }, "mainnet", &treasury).is_err(), "amount differs");
        assert!(check_tx(&tx(&m), &m, "testnet", &treasury).is_err(), "wrong network contracts");
        assert!(check_tx(&tx(&m), &m, "mainnet", "someone.near").is_err(), "not from the treasury");
        let big = Move::Register { storage_yocto: MAX_LENDING_STORAGE_YOCTO + 1 };
        assert!(check_tx(&tx(&big), &big, "mainnet", &treasury).is_err(), "storage bound");
        assert_eq!(lending_contract("testnet"), "contract.1689937928.burrow.testnet");
    }
}
