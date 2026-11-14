//! Boot as HR: load the master plan, propose and validate the org chart and
//! first tasks, and persist them. Startup is idempotent: if a plan is already
//! stored, it and its org chart are reused and nothing is duplicated.

use std::collections::BTreeMap;

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, TaskPlacement};
use axom_core::domain::{Actor, Capabilities, Department, Plan, Task, Team, Worker, WorkerKind};
use axom_core::state::{TaskStatus, WorkerStatus};
use axom_hr::cap;
use axom_hr::policy::{self, Action, Context, PolicyConfig};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::PlannerConfig;
use crate::planner::{self, Planner};

#[derive(Debug, Clone, Serialize)]
pub struct BootReport {
    pub plan_id: Uuid,
    /// True if an existing stored plan was reused.
    pub reused: bool,
    pub departments: usize,
    pub workers_created: usize,
    pub tasks_created: usize,
}

/// Stable hash of a plan's content.
pub fn plan_hash(plan: &Plan) -> String {
    let json = serde_json::to_vec(plan).expect("plan serializes");
    hex::encode(Sha256::digest(&json))
}

/// Boot from a master plan. See module docs for idempotency.
pub async fn bootstrap(
    pool: &PgPool,
    policy_cfg: &PolicyConfig,
    planner_cfg: &PlannerConfig,
    planner: &dyn Planner,
    plan: &Plan,
) -> anyhow::Result<BootReport> {
    let actor = Actor::System;
    let hash = plan_hash(plan);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('axom-bootstrap', 0))").execute(&mut *tx).await?;

    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, content_hash FROM axom.plans ORDER BY created_at, id LIMIT 1").fetch_optional(&mut *tx).await?;
    if let Some((plan_id, stored_hash)) = existing {
        if stored_hash != hash {
            let ev = AuditEvent::new(&actor, "plan.boot", "reused_stored_plan")
                .inputs(json!({ "plan_id": plan_id, "stored_hash": stored_hash, "file_hash": hash }))
                .reasoning("a plan is already stored; the changed plan file was not applied (replacing a plan is an admin action)");
            audit::append(&mut *tx, &ev).await?;
        }
        let (departments,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.departments WHERE plan_id = $1")
            .bind(plan_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(BootReport { plan_id, reused: true, departments: departments as usize, workers_created: 0, tasks_created: 0 });
    }

    let monthly_cap = cap::current_cap(&mut *tx).await?;
    let proposal = planner.propose(plan).await?;
    if let Err(e) = planner::validate(plan, &proposal, planner_cfg, monthly_cap) {
        tx.rollback().await?;
        let ev = AuditEvent::new(&actor, "plan.boot", "rejected")
            .inputs(json!({ "plan_hash": hash, "errors": e.0 }))
            .reasoning("planner proposal failed validation; nothing was saved")
            .policy_version(policy_cfg.version());
        audit::append(pool, &ev).await?;
        return Err(e.into());
    }

    let kill: Option<(bool,)> = sqlx::query_as("SELECT kill_switch FROM axom.system_controls WHERE id = 1").fetch_optional(&mut *tx).await?;
    let mut ctx = Context::new(actor.clone(), Utc::now());
    ctx.kill_switch = kill.map(|k| k.0).unwrap_or(true);

    let plan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.plans (id, content_hash, goal, budget_usd, deadline, objectives, source, proposal, policy_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(plan_id)
    .bind(&hash)
    .bind(&plan.goal)
    .bind(plan.budget_usd)
    .bind(plan.deadline)
    .bind(serde_json::to_value(&plan.objectives)?)
    .bind(serde_json::to_value(plan)?)
    .bind(serde_json::to_value(&proposal)?)
    .bind(policy_cfg.version())
    .execute(&mut *tx)
    .await?;

    let mut dept_ids = BTreeMap::new();
    let mut workers_created = 0;
    for d in &proposal.departments {
        let dept_id = Uuid::new_v4();
        sqlx::query("INSERT INTO axom.departments (id, plan_id, name) VALUES ($1,$2,$3)")
            .bind(dept_id)
            .bind(plan_id)
            .bind(&d.name)
            .execute(&mut *tx)
            .await?;
        dept_ids.insert(d.name.clone(), dept_id);
        for t in &d.teams {
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO axom.teams (id, department_id, name) VALUES ($1,$2,$3)")
                .bind(team_id)
                .bind(dept_id)
                .bind(&t.name)
                .execute(&mut *tx)
                .await?;
            for r in &t.roles {
                let role_id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO axom.roles (id, team_id, title, kind, skills, tool_allowlist, pay_rate_usd, pay_unit, kpis, headcount)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                )
                .bind(role_id)
                .bind(team_id)
                .bind(&r.title)
                .bind(r.kind.as_str())
                .bind(&r.skills)
                .bind(&r.tools)
                .bind(r.pay_rate_usd)
                .bind(r.pay_unit.as_str())
                .bind(serde_json::to_value(&r.kpis)?)
                .bind(r.headcount as i32)
                .execute(&mut *tx)
                .await?;

                // AI agents are hired autonomously at boot. Human roles are
                // filled through hiring requests (milestone 8).
                if r.kind != WorkerKind::AiAgent {
                    continue;
                }
                let decision = policy::evaluate(
                    policy_cfg,
                    &Action::Hire { kind: WorkerKind::AiAgent, role: r.title.clone(), contract_template: None },
                    &ctx,
                );
                for n in 1..=r.headcount {
                    let name = format!("{} #{n}", r.title);
                    if !decision.is_allowed() {
                        let ev = AuditEvent::new(&actor, "worker.hire", "denied")
                            .inputs(json!({ "role": r.title, "name": name }))
                            .reasoning(&decision.reason)
                            .policy_version(&decision.policy_version);
                        audit::append(&mut *tx, &ev).await?;
                        continue;
                    }
                    let w = Worker {
                        id: Uuid::new_v4(),
                        name: name.clone(),
                        kind: WorkerKind::AiAgent,
                        role: r.title.clone(),
                        status: WorkerStatus::Active,
                        skills: r.skills.iter().cloned().collect(),
                        tool_allowlist: r.tools.iter().cloned().collect(),
                        pay_rate_usd: r.pay_rate_usd,
                        pay_unit: r.pay_unit,
                        wallet_address: None,
                        kpis: r.kpis.clone(),
                    };
                    db::insert_worker(&mut *tx, Some(team_id), Some(role_id), &w).await?;
                    let ev = AuditEvent::new(&actor, "worker.hire", "hired")
                        .inputs(json!({ "worker_id": w.id, "name": name, "kind": "ai_agent", "role": r.title,
                                        "tools": r.tools, "skills": r.skills, "pay_rate_usd": r.pay_rate_usd }))
                        .reasoning(&decision.reason)
                        .policy_version(&decision.policy_version);
                    audit::append(&mut *tx, &ev).await?;
                    workers_created += 1;
                }
            }
        }
    }

    for (seq, t) in proposal.tasks.iter().enumerate() {
        let task = Task {
            id: Uuid::new_v4(),
            objective: t.objective.clone(),
            required_capabilities: Capabilities::new(t.required_tools.clone(), t.required_skills.clone()),
            priority: t.priority,
            deadline: Some(plan.deadline),
            assignee: None,
            status: TaskStatus::Queued,
            result: None,
            cost_usd: Decimal::ZERO,
            start_at: None,
        };
        let placement = TaskPlacement { plan_id: Some(plan_id), department_id: dept_ids.get(&t.department).copied(), seq: seq as i32 };
        db::insert_task(&mut *tx, &task, &placement).await?;
    }

    let ev = AuditEvent::new(&actor, "plan.boot", "created")
        .inputs(json!({ "plan_id": plan_id, "plan_hash": hash, "planner": proposal.planner,
                        "departments": proposal.departments.len(), "tasks": proposal.tasks.len(), "workers_created": workers_created }))
        .reasoning("planner proposal validated and saved")
        .policy_version(policy_cfg.version());
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;

    Ok(BootReport {
        plan_id,
        reused: false,
        departments: proposal.departments.len(),
        workers_created,
        tasks_created: proposal.tasks.len(),
    })
}

/// The stored plan id, if any.
pub async fn current_plan_id(pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM axom.plans ORDER BY created_at, id LIMIT 1").fetch_optional(pool).await?;
    Ok(row.map(|r| r.0))
}

/// Load the org chart (departments, teams, workers) for a plan.
pub async fn load_org_chart(pool: &PgPool, plan_id: Uuid) -> Result<Vec<Department>, sqlx::Error> {
    let teams = sqlx::query(
        "SELECT d.id AS dept_id, d.name AS dept_name, t.id AS team_id, t.name AS team_name
         FROM axom.departments d LEFT JOIN axom.teams t ON t.department_id = d.id
         WHERE d.plan_id = $1 ORDER BY d.name, t.name",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await?;
    let worker_teams: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, team_id FROM axom.workers").fetch_all(pool).await?;
    let workers = db::load_workers(pool).await?;

    let mut out: Vec<Department> = Vec::new();
    for r in teams {
        let dept_id: Uuid = r.try_get("dept_id")?;
        if out.last().is_none_or(|d| d.id != dept_id) {
            out.push(Department { id: dept_id, name: r.try_get("dept_name")?, teams: vec![] });
        }
        if let Some(team_id) = r.try_get::<Option<Uuid>, _>("team_id")? {
            let members = workers
                .iter()
                .filter(|w| worker_teams.iter().any(|(id, t)| *id == w.id && *t == Some(team_id)))
                .cloned()
                .collect();
            out.last_mut().expect("pushed above").teams.push(Team { id: team_id, name: r.try_get("team_name")?, workers: members });
        }
    }
    Ok(out)
}
