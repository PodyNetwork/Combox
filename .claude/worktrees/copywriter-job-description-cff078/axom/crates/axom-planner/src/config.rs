//! Versioned planner config (`config/planner.toml`).

use std::path::Path;

use axom_core::domain::{Kpi, PayUnit, WorkerKind};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleTemplate {
    pub title: String,
    pub kind: WorkerKind,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub pay_rate_usd: Decimal,
    pub pay_unit: PayUnit,
    pub headcount: u32,
    #[serde(default)]
    pub kpis: Vec<Kpi>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamTemplate {
    pub name: String,
    pub roles: Vec<RoleTemplate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepartmentTemplate {
    pub name: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Capabilities required by tasks created for this department.
    #[serde(default)]
    pub task_tools: Vec<String>,
    #[serde(default)]
    pub task_skills: Vec<String>,
    pub teams: Vec<TeamTemplate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerConfig {
    pub version: String,
    pub fallback_department: String,
    pub known_tools: Vec<String>,
    pub departments: Vec<DepartmentTemplate>,
}

impl PlannerConfig {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Self = toml::from_str(text)?;
        if !cfg.departments.iter().any(|d| d.name == cfg.fallback_department) {
            anyhow::bail!("fallback_department {} has no template", cfg.fallback_department);
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn department(&self, name: &str) -> Option<&DepartmentTemplate> {
        self.departments.iter().find(|d| d.name.eq_ignore_ascii_case(name))
    }

    pub fn is_known_tool(&self, tool: &str) -> bool {
        self.known_tools.iter().any(|t| t == tool)
    }
}
