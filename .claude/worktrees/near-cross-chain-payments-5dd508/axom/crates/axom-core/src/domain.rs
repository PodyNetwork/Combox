//! Domain model.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{PayoutStatus, TaskStatus, WorkerStatus};

/// Error returned when parsing an enum from its database string.
#[derive(Debug, thiserror::Error)]
#[error("unknown {kind} value: {value}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

/// Implements `as_str`, `Display` and `FromStr` for a unit enum with
/// snake_case database values.
#[macro_export]
macro_rules! str_enum {
    ($ty:ident, $label:literal, { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$variant),+];
            pub fn as_str(&self) -> &'static str {
                match self { $($ty::$variant => $s),+ }
            }
        }
        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl ::std::str::FromStr for $ty {
            type Err = $crate::domain::ParseEnumError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($ty::$variant),)+
                    other => Err($crate::domain::ParseEnumError { kind: $label, value: other.to_string() }),
                }
            }
        }
    };
}

/// Whether a worker is an AI agent or a human freelancer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    /// Does work by calling tools.
    AiAgent,
    /// Hired via Upwork, receives briefs through task intake, never executes tools.
    HumanFreelancer,
}
str_enum!(WorkerKind, "worker kind", { AiAgent => "ai_agent", HumanFreelancer => "human_freelancer" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayUnit {
    Task,
    Hour,
}
str_enum!(PayUnit, "pay unit", { Task => "task", Hour => "hour" });

/// A key performance indicator with a target value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kpi {
    pub name: String,
    pub target: f64,
    /// `true` if higher values are better.
    #[serde(default = "default_true")]
    pub higher_is_better: bool,
}

fn default_true() -> bool {
    true
}

/// One objective of a master plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Optional hint naming the department that owns the objective.
    #[serde(default)]
    pub department: Option<String>,
}

/// The master plan ForeMan boots from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    pub budget_usd: Decimal,
    pub deadline: NaiveDate,
    pub objectives: Vec<Objective>,
}

/// Capabilities a task needs: tools and skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub tools: BTreeSet<String>,
    #[serde(default)]
    pub skills: BTreeSet<String>,
}

impl Capabilities {
    pub fn new<T, S>(tools: T, skills: S) -> Self
    where
        T: IntoIterator,
        T::Item: Into<String>,
        S: IntoIterator,
        S::Item: Into<String>,
    {
        Self {
            tools: tools.into_iter().map(Into::into).collect(),
            skills: skills.into_iter().map(Into::into).collect(),
        }
    }

    /// A task that needs any tool is a tool-execution task. Human workers
    /// never receive those.
    pub fn is_tool_execution(&self) -> bool {
        !self.tools.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Department {
    pub id: Uuid,
    pub name: String,
    pub teams: Vec<Team>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub workers: Vec<Worker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    pub id: Uuid,
    pub name: String,
    pub kind: WorkerKind,
    pub role: String,
    pub status: WorkerStatus,
    pub skills: BTreeSet<String>,
    pub tool_allowlist: BTreeSet<String>,
    pub pay_rate_usd: Decimal,
    pub pay_unit: PayUnit,
    pub wallet_address: Option<String>,
    pub kpis: Vec<Kpi>,
}

impl Worker {
    /// Whether the worker's tools and skills cover the required capabilities.
    /// A human worker never covers a tool-execution task.
    pub fn covers(&self, caps: &Capabilities) -> bool {
        if caps.is_tool_execution() && self.kind == WorkerKind::HumanFreelancer {
            return false;
        }
        caps.tools.is_subset(&self.tool_allowlist) && caps.skills.is_subset(&self.skills)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub objective: String,
    pub required_capabilities: Capabilities,
    /// 1 (highest) to 5 (lowest).
    pub priority: i32,
    pub deadline: Option<NaiveDate>,
    pub assignee: Option<Uuid>,
    pub status: TaskStatus,
    pub result: Option<serde_json::Value>,
    pub cost_usd: Decimal,
    /// When work on the task may start. Nothing routes or runs it before
    /// then. `None` on a new task means now (the database fills it in).
    #[serde(default)]
    pub start_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payout {
    pub id: Uuid,
    pub worker_id: Option<Uuid>,
    pub recipient: String,
    pub amount_usd: Decimal,
    pub fee_usd: Decimal,
    pub status: PayoutStatus,
    pub status_reason: String,
    pub policy_version: String,
    pub reservation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl Payout {
    /// Amount counted against the monthly cap: payout plus network fee.
    pub fn cap_amount(&self) -> Decimal {
        self.amount_usd + self.fee_usd
    }
}

/// Who performed an action. Agents can only propose; some actions are
/// reserved for MFA-verified human admins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    /// ForeMan itself (deterministic services).
    System,
    /// An LLM-driven agent (e.g. the HR agent or a worker agent).
    Agent { name: String },
    /// A named human admin. `mfa_verified` comes from the authenticated session.
    Admin { name: String, mfa_verified: bool },
}

impl Actor {
    pub fn agent(name: impl Into<String>) -> Self {
        Actor::Agent { name: name.into() }
    }

    pub fn is_mfa_admin(&self) -> bool {
        matches!(self, Actor::Admin { mfa_verified: true, .. })
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::System => f.write_str("system"),
            Actor::Agent { name } => write!(f, "agent:{name}"),
            Actor::Admin { name, mfa_verified } => {
                write!(f, "admin:{name}{}", if *mfa_verified { "" } else { "(no-mfa)" })
            }
        }
    }
}

/// Parse a string-backed enum from a database row value.
pub fn parse_db<T: FromStr<Err = ParseEnumError>>(s: &str) -> Result<T, sqlx::Error> {
    s.parse::<T>().map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn worker(kind: WorkerKind, tools: &[&str], skills: &[&str]) -> Worker {
        Worker {
            id: Uuid::new_v4(),
            name: "w".into(),
            kind,
            role: "r".into(),
            status: WorkerStatus::Active,
            skills: skills.iter().map(|s| s.to_string()).collect(),
            tool_allowlist: tools.iter().map(|s| s.to_string()).collect(),
            pay_rate_usd: Decimal::ONE,
            pay_unit: PayUnit::Task,
            wallet_address: None,
            kpis: vec![],
        }
    }

    #[test]
    fn covers_requires_tools_and_skills() {
        let w = worker(WorkerKind::AiAgent, &["postiz", "smtp"], &["copywriting"]);
        assert!(w.covers(&Capabilities::new(["postiz"], ["copywriting"])));
        assert!(!w.covers(&Capabilities::new(["crm"], ["copywriting"])));
        assert!(!w.covers(&Capabilities::new(["postiz"], ["design"])));
    }

    #[test]
    fn humans_never_cover_tool_execution() {
        let mut w = worker(WorkerKind::HumanFreelancer, &[], &["design"]);
        assert!(w.covers(&Capabilities::new(Vec::<String>::new(), ["design"])));
        // Even with a (misconfigured) allowlist, tool tasks are refused.
        w.tool_allowlist.insert("postiz".into());
        assert!(!w.covers(&Capabilities::new(["postiz"], ["design"])));
    }

    #[test]
    fn enum_round_trip() {
        for k in WorkerKind::ALL {
            assert_eq!(k.as_str().parse::<WorkerKind>().unwrap(), *k);
        }
        assert!("robot".parse::<WorkerKind>().is_err());
    }
}
