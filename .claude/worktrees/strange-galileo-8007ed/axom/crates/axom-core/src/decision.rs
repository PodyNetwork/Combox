//! Policy decision types, shared by the policy engine (axom-hr) and the
//! adapters (axom-tools), which refuse to act without an `allow` decision.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allow,
    RequireApproval,
    Deny,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Allow => "allow",
            Outcome::RequireApproval => "require_approval",
            Outcome::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub outcome: Outcome,
    pub reason: String,
    pub policy_version: String,
    /// The owner must be notified when this action executes.
    pub notify_owner: bool,
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        self.outcome == Outcome::Allow
    }
}


/// A NEAR/USD price quote from the configured feed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceQuote {
    pub usd_per_near: Decimal,
    pub fetched_at: DateTime<Utc>,
}
