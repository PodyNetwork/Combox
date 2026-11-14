//! Plan decomposition. A planner produces a [`PlanProposal`]; the proposal is
//! validated before anything is saved. The default planner is rule-based and
//! deterministic; an LLM planner can implement the same trait later, and its
//! output goes through the same validation.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use async_trait::async_trait;
use axom_core::domain::{Kpi, PayUnit, Plan, WorkerKind};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::PlannerConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleProposal {
    pub title: String,
    pub kind: WorkerKind,
    pub skills: Vec<String>,
    pub tools: Vec<String>,
    pub pay_rate_usd: Decimal,
    pub pay_unit: PayUnit,
    pub headcount: u32,
    pub kpis: Vec<Kpi>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamProposal {
    pub name: String,
    pub roles: Vec<RoleProposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepartmentProposal {
    pub name: String,
    pub teams: Vec<TeamProposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskProposal {
    pub objective: String,
    pub department: String,
    pub required_tools: Vec<String>,
    pub required_skills: Vec<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanProposal {
    pub planner: String,
    pub departments: Vec<DepartmentProposal>,
    pub tasks: Vec<TaskProposal>,
}

#[async_trait]
pub trait Planner: Send + Sync {
    async fn propose(&self, plan: &Plan) -> anyhow::Result<PlanProposal>;
}

/// Deterministic keyword planner driven by `config/planner.toml`.
pub struct RuleBasedPlanner {
    pub config: PlannerConfig,
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

impl RuleBasedPlanner {
    pub fn new(config: PlannerConfig) -> Self {
        Self { config }
    }

    /// Pick the department for an objective: an explicit hint that names a
    /// template wins; otherwise most keyword hits (ties by config order);
    /// otherwise the fallback department.
    pub fn classify(&self, title: &str, description: &str, hint: Option<&str>) -> String {
        if let Some(d) = hint.and_then(|h| self.config.department(h)) {
            return d.name.clone();
        }
        let words = tokens(&format!("{title} {description}"));
        let mut best: Option<(&str, usize)> = None;
        for d in &self.config.departments {
            let hits = d.keywords.iter().filter(|k| words.contains(&k.to_lowercase())).count();
            if hits > 0 && best.is_none_or(|(_, h)| hits > h) {
                best = Some((&d.name, hits));
            }
        }
        best.map(|(n, _)| n.to_string()).unwrap_or_else(|| self.config.fallback_department.clone())
    }
}

#[async_trait]
impl Planner for RuleBasedPlanner {
    async fn propose(&self, plan: &Plan) -> anyhow::Result<PlanProposal> {
        let mut used: BTreeMap<usize, ()> = BTreeMap::new();
        let mut tasks = Vec::new();
        for (i, obj) in plan.objectives.iter().enumerate() {
            let dept_name = self.classify(&obj.title, &obj.description, obj.department.as_deref());
            let (idx, tmpl) = self
                .config
                .departments
                .iter()
                .enumerate()
                .find(|(_, d)| d.name == dept_name)
                .expect("classify returns a configured department");
            used.insert(idx, ());
            let objective = if obj.description.is_empty() {
                obj.title.clone()
            } else {
                format!("{}: {}", obj.title, obj.description)
            };
            tasks.push(TaskProposal {
                objective,
                department: tmpl.name.clone(),
                required_tools: tmpl.task_tools.clone(),
                required_skills: tmpl.task_skills.clone(),
                // Earlier objectives first: 1, 1, 2, 2, 3, ... capped at 5.
                priority: (1 + (i as i32) / 2).min(5),
            });
        }
        let departments = used
            .keys()
            .map(|&i| {
                let d = &self.config.departments[i];
                DepartmentProposal {
                    name: d.name.clone(),
                    teams: d
                        .teams
                        .iter()
                        .map(|t| TeamProposal {
                            name: t.name.clone(),
                            roles: t
                                .roles
                                .iter()
                                .map(|r| RoleProposal {
                                    title: r.title.clone(),
                                    kind: r.kind,
                                    skills: r.skills.clone(),
                                    tools: r.tools.clone(),
                                    pay_rate_usd: r.pay_rate_usd,
                                    pay_unit: r.pay_unit,
                                    headcount: r.headcount,
                                    kpis: r.kpis.clone(),
                                })
                                .collect(),
                        })
                        .collect(),
                }
            })
            .collect();
        Ok(PlanProposal { planner: format!("rule_based@{}", self.config.version), departments, tasks })
    }
}

/// Validation failures for a plan or proposal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid plan proposal: {}", .0.join("; "))]
pub struct ValidationError(pub Vec<String>);

/// Maximum workers per role a proposal may request.
pub const MAX_HEADCOUNT_PER_ROLE: u32 = 10;

/// Validate the plan itself and the planner's proposal. Plan budgets above
/// the monthly cap are rejected: plans cannot raise the cap.
pub fn validate(plan: &Plan, proposal: &PlanProposal, cfg: &PlannerConfig, monthly_cap: Decimal) -> Result<(), ValidationError> {
    let mut errs = Vec::new();
    if plan.goal.trim().is_empty() {
        errs.push("plan goal is empty".into());
    }
    if plan.objectives.is_empty() {
        errs.push("plan has no objectives".into());
    }
    if plan.budget_usd < Decimal::ZERO {
        errs.push("plan budget is negative".into());
    }
    if plan.budget_usd > monthly_cap {
        errs.push(format!("plan budget {} USD exceeds the monthly cap of {monthly_cap} USD", plan.budget_usd));
    }
    if proposal.tasks.is_empty() {
        errs.push("proposal has no tasks".into());
    }

    let mut dept_names = HashSet::new();
    for d in &proposal.departments {
        if d.name.trim().is_empty() || !dept_names.insert(d.name.as_str()) {
            errs.push(format!("department name {:?} is empty or duplicated", d.name));
        }
        let mut team_names = HashSet::new();
        for t in &d.teams {
            if t.name.trim().is_empty() || !team_names.insert(t.name.as_str()) {
                errs.push(format!("team name {:?} in {} is empty or duplicated", t.name, d.name));
            }
            let mut titles = HashSet::new();
            for r in &t.roles {
                if r.title.trim().is_empty() || !titles.insert(r.title.as_str()) {
                    errs.push(format!("role title {:?} in {} is empty or duplicated", r.title, t.name));
                }
                if r.kind == WorkerKind::HumanFreelancer && !r.tools.is_empty() {
                    errs.push(format!("human role {} may not have tools", r.title));
                }
                for tool in &r.tools {
                    if !cfg.is_known_tool(tool) {
                        errs.push(format!("role {} uses unknown tool {tool}", r.title));
                    }
                }
                if r.pay_rate_usd < Decimal::ZERO {
                    errs.push(format!("role {} has a negative pay rate", r.title));
                }
                if r.headcount > MAX_HEADCOUNT_PER_ROLE {
                    errs.push(format!("role {} headcount {} exceeds {MAX_HEADCOUNT_PER_ROLE}", r.title, r.headcount));
                }
            }
        }
    }
    for t in &proposal.tasks {
        if t.objective.trim().is_empty() {
            errs.push("task with empty objective".into());
        }
        if !dept_names.contains(t.department.as_str()) {
            errs.push(format!("task {:?} references unknown department {}", t.objective, t.department));
        }
        if !(1..=5).contains(&t.priority) {
            errs.push(format!("task {:?} priority {} out of range", t.objective, t.priority));
        }
        for tool in &t.required_tools {
            if !cfg.is_known_tool(tool) {
                errs.push(format!("task {:?} requires unknown tool {tool}", t.objective));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(ValidationError(errs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axom_core::domain::Objective;
    use rust_decimal_macros::dec;

    pub(crate) fn repo_config() -> PlannerConfig {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/planner.toml");
        PlannerConfig::load(&path).unwrap()
    }

    fn plan() -> Plan {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/master_plan.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn classifies_by_keywords_hint_and_fallback() {
        let p = RuleBasedPlanner::new(repo_config());
        assert_eq!(p.classify("Build the MVP web app", "", None), "Engineering");
        assert_eq!(p.classify("Design the brand and logo", "", None), "Design");
        assert_eq!(p.classify("Something vague", "", None), "Operations");
        assert_eq!(p.classify("Build the MVP", "", Some("sales")), "Sales");
        assert_eq!(p.classify("Build the MVP", "", Some("nonexistent")), "Engineering");
    }

    #[tokio::test]
    async fn sample_plan_decomposes_and_validates() {
        let cfg = repo_config();
        let p = RuleBasedPlanner::new(cfg.clone());
        let plan = plan();
        let prop = p.propose(&plan).await.unwrap();
        validate(&plan, &prop, &cfg, dec!(500)).unwrap();
        let depts: Vec<&str> = prop.departments.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(depts, ["Engineering", "Marketing", "Sales", "Design", "Operations"]);
        assert_eq!(prop.tasks.len(), plan.objectives.len());
        assert_eq!(prop.tasks[0].priority, 1);
        // Same input, same output.
        assert_eq!(prop, p.propose(&plan).await.unwrap());
    }

    #[tokio::test]
    async fn rejects_budget_over_cap_and_bad_roles() {
        let cfg = repo_config();
        let p = RuleBasedPlanner::new(cfg.clone());
        let mut plan = plan();
        plan.budget_usd = dec!(5000);
        let mut prop = p.propose(&plan).await.unwrap();
        prop.departments[0].teams[0].roles[0].tools.push("rm_rf".into());
        prop.departments[3].teams[0].roles[0].tools.push("postiz".into());
        let err = validate(&plan, &prop, &cfg, dec!(500)).unwrap_err();
        assert!(err.0.iter().any(|e| e.contains("exceeds the monthly cap")), "{err}");
        assert!(err.0.iter().any(|e| e.contains("unknown tool rm_rf")), "{err}");
        assert!(err.0.iter().any(|e| e.contains("human role Brand Designer may not have tools")), "{err}");
    }

    #[test]
    fn empty_plan_rejected() {
        let cfg = repo_config();
        let plan = Plan { goal: "".into(), budget_usd: dec!(1), deadline: chrono::NaiveDate::MIN, objectives: Vec::<Objective>::new() };
        let prop = PlanProposal { planner: "x".into(), departments: vec![], tasks: vec![] };
        assert_eq!(validate(&plan, &prop, &cfg, dec!(500)).unwrap_err().0.len(), 3);
    }
}
