//! Deterministic policy engine. LLM output is only ever a proposal; every
//! action that moves money, hires, fires, sends a contract, or sends an
//! external message is evaluated here first.

use std::path::Path;

use axom_core::domain::{Actor, WorkerKind};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum PolicyConfigError {
    #[error("reading policy file: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing policy file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid {var}: {msg}")]
    Env { var: &'static str, msg: String },
    #[error("invalid policy: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PayoutRules {
    pub approval_threshold_usd: Decimal,
    pub aggregation_window_hours: i64,
    pub price_max_age_secs: i64,
    pub approval_expiry_days: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapRules {
    pub default_monthly_cap_usd: Decimal,
}

/// Cross-chain payouts through NEAR Intents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentRules {
    /// Estimated route cost (1Click fee plus destination-chain withdrawal),
    /// in basis points of the payout. Added to the NEAR deposit so the worker
    /// receives what they are owed, and counted toward the cap as a fee.
    pub route_fee_bps: u32,
    /// Slippage tolerance sent with every quote, in basis points.
    pub slippage_bps: u32,
    /// The signer refuses a quote whose USD output is below the owed amount
    /// by more than this many basis points (price moved, route too costly).
    pub max_shortfall_bps: u32,
}

impl Default for IntentRules {
    fn default() -> Self {
        Self { route_fee_bps: 100, slippage_bps: 100, max_shortfall_bps: 300 }
    }
}

/// Treasury yield: idle NEAR supplied to Rhea lending (see `treasury`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreasuryRules {
    /// Supply idle NEAR to Rhea lending. Off by default; withdrawals back to
    /// the treasury are always allowed.
    pub yield_enabled: bool,
    /// Supplying stops at this much above the liquidity floor (hysteresis:
    /// withdrawals start only below the floor itself).
    pub liquidity_margin_pct: u32,
    /// Always keep at least this much NEAR liquid (gas, storage, small payouts).
    pub min_liquid_near: Decimal,
    /// Never put more than this share of the treasury in Rhea.
    pub max_deployed_pct: u32,
    /// Don't move less than this (no churn for dust).
    pub min_move_near: Decimal,
}

impl Default for TreasuryRules {
    fn default() -> Self {
        Self {
            yield_enabled: false,
            liquidity_margin_pct: 25,
            min_liquid_near: Decimal::ONE,
            max_deployed_pct: 90,
            min_move_near: Decimal::new(5, 1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractRules {
    pub approved_templates: Vec<String>,
}

/// The policy file as written on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyFile {
    pub version: String,
    pub payouts: PayoutRules,
    pub cap: CapRules,
    pub contracts: ContractRules,
    #[serde(default)]
    pub privacy: PrivacyRules,
    #[serde(default)]
    pub intents: IntentRules,
    #[serde(default)]
    pub treasury: TreasuryRules,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivacyRules {
    /// Days to keep data of candidates who were not hired, then redact it.
    pub candidate_retention_days: i64,
}

impl Default for PrivacyRules {
    fn default() -> Self {
        Self { candidate_retention_days: 180 }
    }
}

/// Effective policy: the file plus environment overrides, with a version
/// string that identifies the exact values used.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyConfig {
    pub file: PolicyFile,
    version: String,
}

impl PolicyConfig {
    pub fn from_file(file: PolicyFile) -> Result<Self, PolicyConfigError> {
        validate(&file)?;
        let json = serde_json::to_vec(&file).expect("policy serializes");
        let hash = hex::encode(Sha256::digest(&json));
        let version = format!("{}+{}", file.version, &hash[..8]);
        Ok(Self { file, version })
    }

    pub fn parse(toml_text: &str) -> Result<Self, PolicyConfigError> {
        Self::from_file(toml::from_str(toml_text)?)
    }

    /// Load from a file, applying `PAYOUT_APPROVAL_THRESHOLD_USD` and
    /// `MONTHLY_PAYOUT_CAP_USD` from the environment when set.
    pub fn load(path: &Path) -> Result<Self, PolicyConfigError> {
        let mut file: PolicyFile = toml::from_str(&std::fs::read_to_string(path)?)?;
        if let Some(v) = env_decimal("PAYOUT_APPROVAL_THRESHOLD_USD")? {
            file.payouts.approval_threshold_usd = v;
        }
        if let Some(v) = env_decimal("MONTHLY_PAYOUT_CAP_USD")? {
            file.cap.default_monthly_cap_usd = v;
        }
        // Judge mode and demos turn treasury yield on without editing the file.
        if std::env::var("TREASURY_YIELD_ENABLED").as_deref() == Ok("true") {
            file.treasury.yield_enabled = true;
        }
        Self::from_file(file)
    }

    /// `"<file version>+<hash>"`, recorded with every decision.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn payouts(&self) -> &PayoutRules {
        &self.file.payouts
    }

    pub fn intents(&self) -> &IntentRules {
        &self.file.intents
    }

    pub fn treasury(&self) -> &TreasuryRules {
        &self.file.treasury
    }

    pub fn approval_expiry(&self) -> Duration {
        Duration::days(self.file.payouts.approval_expiry_days)
    }

    pub fn aggregation_window(&self) -> Duration {
        Duration::hours(self.file.payouts.aggregation_window_hours)
    }

    pub fn is_approved_template(&self, template: &str) -> bool {
        self.file.contracts.approved_templates.iter().any(|t| t == template)
    }
}

fn env_decimal(var: &'static str) -> Result<Option<Decimal>, PolicyConfigError> {
    match std::env::var(var) {
        Ok(s) if !s.trim().is_empty() => s
            .trim()
            .parse::<Decimal>()
            .map(Some)
            .map_err(|e| PolicyConfigError::Env { var, msg: e.to_string() }),
        _ => Ok(None),
    }
}

fn validate(f: &PolicyFile) -> Result<(), PolicyConfigError> {
    let bad = |m: &str| Err(PolicyConfigError::Invalid(m.to_string()));
    if f.version.trim().is_empty() {
        return bad("version must be set");
    }
    if f.payouts.approval_threshold_usd <= Decimal::ZERO {
        return bad("approval_threshold_usd must be positive");
    }
    if f.payouts.aggregation_window_hours <= 0 || f.payouts.price_max_age_secs <= 0 || f.payouts.approval_expiry_days <= 0 {
        return bad("durations must be positive");
    }
    let t = &f.treasury;
    if t.liquidity_margin_pct > 1_000 || t.max_deployed_pct > 100 || t.min_liquid_near < Decimal::ZERO || t.min_move_near <= Decimal::ZERO {
        return bad("treasury: margin at most 1000%, max_deployed_pct at most 100, min_liquid_near >= 0, min_move_near > 0");
    }
    if f.intents.route_fee_bps > 2_000 || f.intents.slippage_bps > 2_000 || f.intents.max_shortfall_bps > 2_000 {
        return bad("intents basis points must be at most 2000 (20%)");
    }
    if f.cap.default_monthly_cap_usd < Decimal::ZERO {
        return bad("default_monthly_cap_usd must not be negative");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

pub use axom_core::decision::{Decision, Outcome};

pub use axom_core::decision::PriceQuote;

/// Actions the policy engine rules on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Payout { recipient: String, amount_usd: Decimal, fee_usd: Decimal },
    Hire { kind: WorkerKind, role: String, contract_template: Option<String> },
    Fire { worker_id: uuid::Uuid, reason: String },
    SendContract { template: String },
    SendExternalMessage { recipients: Vec<String>, allowed_recipients: Vec<String> },
    ExecuteTool { tool: String, worker_allowlist: Vec<String> },
    /// Meeting operations (create, dispatch agent, participant join, end).
    Meeting { op: String },
    /// An alert to the owner (email + dashboard). Allowed even under the kill
    /// switch, so the owner always hears about what happened.
    NotifyOwner { recipient: String, owner_email: String },
    // Protected actions: only an MFA-verified human admin, via the dashboard.
    ApprovePayout { payout_id: uuid::Uuid },
    ChangeCap { new_cap_usd: Decimal },
    ChangePolicy,
    ChangeToolAllowlist { worker_id: uuid::Uuid },
    ChangeWalletSettings,
    ToggleKillSwitch { on: bool },
    /// Move treasury NEAR between its own account and its own Rhea lending
    /// position (`register`, `supply`, `withdraw`, `unwrap`).
    TreasuryMove { kind: String, amount_near: Decimal },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Payout { .. } => "payout",
            Action::Hire { .. } => "hire",
            Action::Fire { .. } => "fire",
            Action::SendContract { .. } => "send_contract",
            Action::SendExternalMessage { .. } => "send_external_message",
            Action::ExecuteTool { .. } => "execute_tool",
            Action::NotifyOwner { .. } => "notify_owner",
            Action::Meeting { .. } => "meeting",
            Action::ApprovePayout { .. } => "approve_payout",
            Action::ChangeCap { .. } => "change_cap",
            Action::ChangePolicy => "change_policy",
            Action::ChangeToolAllowlist { .. } => "change_tool_allowlist",
            Action::ChangeWalletSettings => "change_wallet_settings",
            Action::ToggleKillSwitch { .. } => "toggle_kill_switch",
            Action::TreasuryMove { .. } => "treasury_move",
        }
    }

    /// Actions that only an MFA-verified admin may perform.
    pub fn is_protected(&self) -> bool {
        matches!(
            self,
            Action::ApprovePayout { .. }
                | Action::ChangeCap { .. }
                | Action::ChangePolicy
                | Action::ChangeToolAllowlist { .. }
                | Action::ChangeWalletSettings
                | Action::ToggleKillSwitch { .. }
        )
    }
}

/// Facts the engine needs that come from the database or the price feed.
#[derive(Debug, Clone)]
pub struct Context {
    pub actor: Actor,
    pub now: DateTime<Utc>,
    pub kill_switch: bool,
    pub price: Option<PriceQuote>,
    /// Sum of live payouts (USD) to the same recipient in the rolling window,
    /// excluding the payout being evaluated.
    pub recipient_window_usd: Decimal,
}

impl Context {
    pub fn new(actor: Actor, now: DateTime<Utc>) -> Self {
        Self { actor, now, kill_switch: false, price: None, recipient_window_usd: Decimal::ZERO }
    }
}

/// Evaluate an action. Pure and deterministic.
pub fn evaluate(cfg: &PolicyConfig, action: &Action, ctx: &Context) -> Decision {
    let d = |outcome, reason: String, notify_owner| Decision {
        outcome,
        reason,
        policy_version: cfg.version().to_string(),
        notify_owner,
    };

    if action.is_protected() {
        return if ctx.actor.is_mfa_admin() {
            d(Outcome::Allow, format!("{} by MFA-verified admin", action.name()), false)
        } else {
            d(
                Outcome::Deny,
                format!("{} is reserved for an MFA-verified admin in the dashboard; actor {} may not perform it", action.name(), ctx.actor),
                false,
            )
        };
    }

    if let Action::NotifyOwner { recipient, owner_email } = action {
        return if !owner_email.trim().is_empty() && recipient.eq_ignore_ascii_case(owner_email) {
            d(Outcome::Allow, "owner alert to the configured owner address".into(), false)
        } else {
            d(Outcome::Deny, format!("owner alerts go only to the owner address, not {recipient}"), false)
        };
    }

    if ctx.kill_switch {
        return d(Outcome::Deny, "kill switch is on: all tool calls, payouts, and outbound actions are halted".into(), false);
    }

    match action {
        Action::Payout { recipient, amount_usd, fee_usd } => {
            let rules = cfg.payouts();
            if recipient.trim().is_empty() {
                return d(Outcome::Deny, "payout has no recipient".into(), false);
            }
            if *amount_usd <= Decimal::ZERO || *fee_usd < Decimal::ZERO {
                return d(Outcome::Deny, "payout amount must be positive and fee non-negative".into(), false);
            }
            match ctx.price {
                None => return d(Outcome::RequireApproval, "no NEAR price available; approval required".into(), false),
                Some(q) => {
                    let age = ctx.now - q.fetched_at;
                    if age > Duration::seconds(rules.price_max_age_secs) || age < Duration::zero() {
                        return d(
                            Outcome::RequireApproval,
                            format!("NEAR price is stale ({}s old, limit {}s); approval required", age.num_seconds(), rules.price_max_age_secs),
                            false,
                        );
                    }
                }
            }
            let aggregate = ctx.recipient_window_usd + amount_usd;
            if aggregate >= rules.approval_threshold_usd {
                d(
                    Outcome::RequireApproval,
                    format!(
                        "recipient total {aggregate} USD over {}h (this payout {amount_usd}) is at or above the {} USD threshold",
                        rules.aggregation_window_hours, rules.approval_threshold_usd
                    ),
                    false,
                )
            } else {
                d(
                    Outcome::Allow,
                    format!("recipient total {aggregate} USD over {}h is under the {} USD threshold", rules.aggregation_window_hours, rules.approval_threshold_usd),
                    false,
                )
            }
        }
        Action::Hire { kind: WorkerKind::AiAgent, role, .. } => {
            d(Outcome::Allow, format!("autonomous AI agent hire for role {role}; included in daily digest"), false)
        }
        Action::Hire { kind: WorkerKind::HumanFreelancer, role, contract_template } => match contract_template {
            Some(t) if cfg.is_approved_template(t) => d(
                Outcome::Allow,
                format!("autonomous freelancer hire for role {role} with approved template {t}; owner notified when contract goes out"),
                true,
            ),
            Some(t) => d(Outcome::Deny, format!("contract template {t} is not owner-approved"), false),
            None => d(Outcome::Deny, "human hire requires an owner-approved contract template".into(), false),
        },
        Action::Fire { reason, .. } => {
            if reason.trim().is_empty() {
                d(Outcome::Deny, "firing requires a recorded reason".into(), false)
            } else {
                d(Outcome::Allow, "autonomous firing; owner notification mandatory".into(), true)
            }
        }
        Action::SendContract { template } => {
            if cfg.is_approved_template(template) {
                d(Outcome::Allow, format!("template {template} is owner-approved"), true)
            } else {
                d(Outcome::Deny, format!("contract template {template} is not owner-approved; the agent may not draft legal language"), false)
            }
        }
        Action::SendExternalMessage { recipients, allowed_recipients } => {
            let outside: Vec<&String> = recipients.iter().filter(|r| !allowed_recipients.contains(r)).collect();
            if recipients.is_empty() {
                d(Outcome::Deny, "message has no recipients".into(), false)
            } else if outside.is_empty() {
                d(Outcome::Allow, "all recipients are on the allowed list".into(), false)
            } else {
                d(Outcome::Deny, format!("recipients not on the allowed list: {outside:?}"), false)
            }
        }
        Action::ExecuteTool { tool, worker_allowlist } => {
            if worker_allowlist.contains(tool) {
                d(Outcome::Allow, format!("tool {tool} is on the worker's allowlist"), false)
            } else {
                d(Outcome::Deny, format!("tool {tool} is not on the worker's allowlist"), false)
            }
        }
        Action::Meeting { op } => d(Outcome::Allow, format!("meeting operation {op}"), false),
        Action::TreasuryMove { kind, amount_near } => {
            // Only the deterministic planner moves treasury funds, never an LLM agent or a person.
            if ctx.actor != Actor::System {
                return d(Outcome::Deny, format!("treasury moves are made only by the treasury planner, not {}", ctx.actor), false);
            }
            if *amount_near <= Decimal::ZERO {
                return d(Outcome::Deny, "treasury move amount must be positive".into(), false);
            }
            match kind.as_str() {
                // Money coming back to the liquid treasury is always allowed.
                "withdraw" | "unwrap" => d(Outcome::Allow, format!("{kind} {amount_near} NEAR back to the treasury"), false),
                "supply" | "register" if cfg.treasury().yield_enabled => {
                    d(Outcome::Allow, format!("{kind} {amount_near} NEAR into the treasury's own Rhea lending position"), false)
                }
                "supply" | "register" => d(Outcome::Deny, "treasury yield is disabled ([treasury] yield_enabled)".into(), false),
                other => d(Outcome::Deny, format!("unknown treasury move {other}"), false),
            }
        }
        Action::NotifyOwner { .. } => unreachable!("handled above"),
        _ => unreachable!("protected actions handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    pub(crate) const TEST_POLICY: &str = r#"
        version = "test.1"
        [payouts]
        approval_threshold_usd = "50"
        aggregation_window_hours = 24
        price_max_age_secs = 900
        approval_expiry_days = 7
        [cap]
        default_monthly_cap_usd = "500"
        [contracts]
        approved_templates = ["freelancer-services-v1"]
    "#;

    fn cfg() -> PolicyConfig {
        PolicyConfig::parse(TEST_POLICY).unwrap()
    }

    fn ctx(now: DateTime<Utc>) -> Context {
        let mut c = Context::new(Actor::System, now);
        c.price = Some(PriceQuote { usd_per_near: dec!(3), fetched_at: now - Duration::minutes(1) });
        c
    }

    fn payout(amount: Decimal) -> Action {
        Action::Payout { recipient: "alice.testnet".into(), amount_usd: amount, fee_usd: dec!(0.01) }
    }

    #[test]
    fn treasury_moves_only_by_the_planner_and_supply_only_when_enabled() {
        let off = cfg();
        let mut file = off.file.clone();
        file.treasury.yield_enabled = true;
        let on = PolicyConfig::from_file(file).unwrap();
        let mv = |kind: &str| Action::TreasuryMove { kind: kind.into(), amount_near: dec!(10) };
        let sys = Context::new(Actor::System, Utc::now());
        assert_eq!(evaluate(&on, &mv("supply"), &sys).outcome, Outcome::Allow);
        assert_eq!(evaluate(&off, &mv("supply"), &sys).outcome, Outcome::Deny, "yield disabled");
        assert_eq!(evaluate(&off, &mv("withdraw"), &sys).outcome, Outcome::Allow, "money coming home is always allowed");
        let agent = Context::new(Actor::agent("hr-agent"), Utc::now());
        assert_eq!(evaluate(&on, &mv("withdraw"), &agent).outcome, Outcome::Deny, "LLM agents never move treasury funds");
        let admin = Context::new(Actor::Admin { name: "o".into(), mfa_verified: true }, Utc::now());
        assert_eq!(evaluate(&on, &mv("supply"), &admin).outcome, Outcome::Deny, "nor do people, outside the planner");
        let mut killed = Context::new(Actor::System, Utc::now());
        killed.kill_switch = true;
        assert_eq!(evaluate(&on, &mv("withdraw"), &killed).outcome, Outcome::Deny, "kill switch halts everything");
        assert_eq!(evaluate(&on, &mv("borrow"), &sys).outcome, Outcome::Deny);
        assert_eq!(evaluate(&on, &Action::TreasuryMove { kind: "supply".into(), amount_near: dec!(0) }, &sys).outcome, Outcome::Deny);
    }

    #[test]
    fn under_threshold_allows() {
        let c = cfg();
        let d = evaluate(&c, &payout(dec!(49.99)), &ctx(Utc::now()));
        assert_eq!(d.outcome, Outcome::Allow);
        assert_eq!(d.policy_version, c.version());
        assert!(d.policy_version.starts_with("test.1+"));
    }

    #[test]
    fn at_or_over_threshold_requires_approval() {
        let c = cfg();
        assert_eq!(evaluate(&c, &payout(dec!(50)), &ctx(Utc::now())).outcome, Outcome::RequireApproval);
        assert_eq!(evaluate(&c, &payout(dec!(120)), &ctx(Utc::now())).outcome, Outcome::RequireApproval);
    }

    #[test]
    fn rolling_window_aggregation_blocks_splitting() {
        let c = cfg();
        let mut x = ctx(Utc::now());
        x.recipient_window_usd = dec!(30);
        let d = evaluate(&c, &payout(dec!(25)), &x);
        assert_eq!(d.outcome, Outcome::RequireApproval, "{}", d.reason);
        x.recipient_window_usd = dec!(20);
        assert_eq!(evaluate(&c, &payout(dec!(25)), &x).outcome, Outcome::Allow);
    }

    #[test]
    fn stale_or_missing_price_requires_approval() {
        let c = cfg();
        let now = Utc::now();
        let mut x = ctx(now);
        x.price = Some(PriceQuote { usd_per_near: dec!(3), fetched_at: now - Duration::minutes(16) });
        assert_eq!(evaluate(&c, &payout(dec!(5)), &x).outcome, Outcome::RequireApproval);
        x.price = Some(PriceQuote { usd_per_near: dec!(3), fetched_at: now - Duration::minutes(15) });
        assert_eq!(evaluate(&c, &payout(dec!(5)), &x).outcome, Outcome::Allow);
        x.price = None;
        assert_eq!(evaluate(&c, &payout(dec!(5)), &x).outcome, Outcome::RequireApproval);
    }

    #[test]
    fn kill_switch_denies_payouts_and_tools() {
        let c = cfg();
        let mut x = ctx(Utc::now());
        x.kill_switch = true;
        assert_eq!(evaluate(&c, &payout(dec!(5)), &x).outcome, Outcome::Deny);
        let tool = Action::ExecuteTool { tool: "postiz".into(), worker_allowlist: vec!["postiz".into()] };
        assert_eq!(evaluate(&c, &tool, &x).outcome, Outcome::Deny);
    }

    #[test]
    fn protected_actions_need_mfa_admin() {
        let c = cfg();
        let now = Utc::now();
        let action = Action::ChangeCap { new_cap_usd: dec!(10000) };
        for actor in [
            Actor::System,
            Actor::agent("hr"),
            Actor::Admin { name: "owner".into(), mfa_verified: false },
        ] {
            let x = Context::new(actor, now);
            assert_eq!(evaluate(&c, &action, &x).outcome, Outcome::Deny);
        }
        let x = Context::new(Actor::Admin { name: "owner".into(), mfa_verified: true }, now);
        assert_eq!(evaluate(&c, &action, &x).outcome, Outcome::Allow);
        let approve = Action::ApprovePayout { payout_id: uuid::Uuid::new_v4() };
        assert_eq!(evaluate(&c, &approve, &Context::new(Actor::agent("hr"), now)).outcome, Outcome::Deny);
    }

    #[test]
    fn contracts_only_from_approved_templates() {
        let c = cfg();
        let x = ctx(Utc::now());
        let ok = Action::SendContract { template: "freelancer-services-v1".into() };
        let bad = Action::SendContract { template: "custom-drafted".into() };
        assert_eq!(evaluate(&c, &ok, &x).outcome, Outcome::Allow);
        assert_eq!(evaluate(&c, &bad, &x).outcome, Outcome::Deny);
        let hire = Action::Hire { kind: WorkerKind::HumanFreelancer, role: "designer".into(), contract_template: None };
        assert_eq!(evaluate(&c, &hire, &x).outcome, Outcome::Deny);
        let hire_ai = Action::Hire { kind: WorkerKind::AiAgent, role: "writer".into(), contract_template: None };
        assert_eq!(evaluate(&c, &hire_ai, &x).outcome, Outcome::Allow);
    }

    #[test]
    fn firing_notifies_owner() {
        let d = evaluate(&cfg(), &Action::Fire { worker_id: uuid::Uuid::new_v4(), reason: "low completion".into() }, &ctx(Utc::now()));
        assert!(d.is_allowed() && d.notify_owner);
    }

    #[test]
    fn external_messages_limited_to_allowed_recipients() {
        let c = cfg();
        let x = ctx(Utc::now());
        let a = Action::SendExternalMessage {
            recipients: vec!["owner@example.com".into(), "attacker@evil.test".into()],
            allowed_recipients: vec!["owner@example.com".into()],
        };
        assert_eq!(evaluate(&c, &a, &x).outcome, Outcome::Deny);
    }

    #[test]
    fn owner_alerts_survive_kill_switch_but_only_to_owner() {
        let c = cfg();
        let mut x = ctx(Utc::now());
        x.kill_switch = true;
        let ok = Action::NotifyOwner { recipient: "Owner@Example.com".into(), owner_email: "owner@example.com".into() };
        assert_eq!(evaluate(&c, &ok, &x).outcome, Outcome::Allow);
        let bad = Action::NotifyOwner { recipient: "x@evil.test".into(), owner_email: "owner@example.com".into() };
        assert_eq!(evaluate(&c, &bad, &x).outcome, Outcome::Deny);
        let unset = Action::NotifyOwner { recipient: "".into(), owner_email: "".into() };
        assert_eq!(evaluate(&c, &unset, &x).outcome, Outcome::Deny);
    }

    #[test]
    fn version_changes_with_values() {
        let a = cfg();
        let mut f = a.file.clone();
        f.payouts.approval_threshold_usd = dec!(40);
        let b = PolicyConfig::from_file(f).unwrap();
        assert_ne!(a.version(), b.version());
    }

    #[test]
    fn repo_policy_file_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml");
        let text = std::fs::read_to_string(path).unwrap();
        let c = PolicyConfig::parse(&text).unwrap();
        assert_eq!(c.payouts().approval_threshold_usd, dec!(50));
        assert_eq!(c.file.cap.default_monthly_cap_usd, dec!(500));
    }
}
