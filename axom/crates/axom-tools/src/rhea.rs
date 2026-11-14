//! Read-only views of the treasury's position: its NEAR balance, its wNEAR
//! balance, and its supplied balance on Rhea lending. Writes go through the
//! signer only.

use std::sync::Mutex;

use async_trait::async_trait;
use axom_core::intents::wnear_contract;
use axom_core::rhea::lending_contract;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};

use crate::near_rpc::{AccountBalance, JsonRpcNear};
use crate::{ToolError, ToolResult};

/// Everything the treasury planner needs, read in one go.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreasuryPosition {
    /// Spendable NEAR (balance minus what storage reserves).
    #[serde(with = "axom_core::near::yocto_str")]
    pub liquid_yocto: u128,
    /// wNEAR held by the treasury (e.g. just withdrawn from Rhea, or an intent refund).
    #[serde(with = "axom_core::near::yocto_str")]
    pub wnear_yocto: u128,
    /// wNEAR supplied on Rhea lending (includes accrued interest).
    #[serde(with = "axom_core::near::yocto_str")]
    pub supplied_yocto: u128,
    /// Whether the treasury is registered on the lending contract.
    pub registered: bool,
    /// The lending contract's storage minimum.
    #[serde(with = "axom_core::near::yocto_str")]
    pub storage_min_yocto: u128,
    /// Current wNEAR supply APR (e.g. 0.0032 = 0.32%).
    pub supply_apr: Decimal,
    /// The lending contract allows deposits / withdrawals of wNEAR.
    pub can_deposit: bool,
    pub can_withdraw: bool,
    /// Internal decimals on top of the token's. Must be 0 for wNEAR.
    pub extra_decimals: u32,
}

#[async_trait]
pub trait TreasuryViews: Send + Sync {
    async fn position(&self, network: &str, treasury: &str) -> ToolResult<TreasuryPosition>;
}

fn u128_at(v: &Value, key: &str) -> u128 {
    v.get(key).and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Parse `get_account` (or `null`) into the supplied wNEAR balance.
pub fn supplied_of(account: &Value, token: &str) -> u128 {
    account
        .get("supplied")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("token_id").and_then(Value::as_str) == Some(token)))
        .map(|x| u128_at(x, "balance"))
        .unwrap_or(0)
}

#[async_trait]
impl TreasuryViews for JsonRpcNear {
    async fn position(&self, network: &str, treasury: &str) -> ToolResult<TreasuryPosition> {
        let lending = lending_contract(network);
        let wnear = wnear_contract(network);
        let liquid_yocto = self.view_account(treasury).await?.as_ref().map(AccountBalance::available_yocto).unwrap_or(0);
        let wnear_yocto = match self.view_function(wnear, "ft_balance_of", &json!({ "account_id": treasury })).await? {
            Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        };
        let account = self.view_function(lending, "get_account", &json!({ "account_id": treasury })).await?;
        let storage = self.view_function(lending, "storage_balance_of", &json!({ "account_id": treasury })).await?;
        let bounds = self.view_function(lending, "storage_balance_bounds", &json!({})).await?;
        let asset = self.view_function(lending, "get_asset", &json!({ "token_id": wnear })).await?;
        if asset.is_null() {
            return Err(ToolError::Permanent(format!("{lending} does not list {wnear}")));
        }
        let cfg = asset.get("config").cloned().unwrap_or(Value::Null);
        Ok(TreasuryPosition {
            liquid_yocto,
            wnear_yocto,
            supplied_yocto: supplied_of(&account, wnear),
            registered: !storage.is_null(),
            storage_min_yocto: u128_at(&bounds, "min"),
            supply_apr: asset.get("supply_apr").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or_default(),
            can_deposit: cfg.get("can_deposit").and_then(Value::as_bool).unwrap_or(false),
            can_withdraw: cfg.get("can_withdraw").and_then(Value::as_bool).unwrap_or(false),
            extra_decimals: cfg.get("extra_decimals").and_then(Value::as_u64).unwrap_or(u64::MAX) as u32,
        })
    }
}

/// In-memory treasury position for tests.
pub struct MockTreasury {
    pub state: Mutex<TreasuryPosition>,
}

impl MockTreasury {
    pub fn new(liquid_yocto: u128) -> Self {
        Self {
            state: Mutex::new(TreasuryPosition {
                liquid_yocto,
                wnear_yocto: 0,
                supplied_yocto: 0,
                registered: false,
                storage_min_yocto: 100_000_000_000_000_000_000_000,
                supply_apr: "0.0032".parse().unwrap(),
                can_deposit: true,
                can_withdraw: true,
                extra_decimals: 0,
            }),
        }
    }

    /// Apply a move the way the chain would (for tests that loop the signer).
    pub fn apply(&self, m: &axom_core::rhea::Move) {
        use axom_core::rhea::Move::*;
        let mut s = self.state.lock().unwrap();
        match *m {
            Register { storage_yocto } => {
                s.liquid_yocto -= storage_yocto;
                s.registered = true;
            }
            Supply { amount_yocto } => {
                s.liquid_yocto -= amount_yocto;
                s.supplied_yocto += amount_yocto;
            }
            Withdraw { amount_yocto } => {
                let a = amount_yocto.min(s.supplied_yocto);
                s.supplied_yocto -= a;
                s.wnear_yocto += a;
            }
            Unwrap { amount_yocto } => {
                s.wnear_yocto -= amount_yocto;
                s.liquid_yocto += amount_yocto;
            }
        }
    }
}

#[async_trait]
impl TreasuryViews for MockTreasury {
    async fn position(&self, _network: &str, _treasury: &str) -> ToolResult<TreasuryPosition> {
        Ok(self.state.lock().unwrap().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supplied_balance() {
        let acct = json!({ "account_id": "t", "supplied": [{ "token_id": "wrap.near", "balance": "123", "shares": "120", "apr": "0.003" }], "collateral": [] });
        assert_eq!(supplied_of(&acct, "wrap.near"), 123);
        assert_eq!(supplied_of(&acct, "usdt.tether-token.near"), 0);
        assert_eq!(supplied_of(&Value::Null, "wrap.near"), 0, "unregistered account");
    }
}
