//! Hiring: AI agents autonomously, human freelancers under policy through
//! Combox's own jobs board (`board`, paid in NEAR) or Upwork (paid through
//! Upwork only).
//!
//! Human flow, each step audited:
//! 1. `post_job`: HR writes a job spec (role, skills, rate, KPIs) for an
//!    open human hiring request; the policy engine checks the hire (it must
//!    name an owner-approved contract template) and then it posts.
//! 2. `shortlist`: applicants are ranked by deterministic skill overlap.
//!    Applicant messages are untrusted data: stored, never interpreted.
//! 3. `invite_to_interview`: a meeting with the AI interviewer (disclosure
//!    and consent first); the join link goes to the candidate on Upwork.
//! 4. `record_interview_score`: the stored rubric scores from the meeting.
//! 5. `send_contract`: an owner-approved DocuSeal template only, never
//!    drafted text; the owner is emailed when the contract goes out.
//! 6. `complete_hire`: once the contract is signed, the worker is activated
//!    (no tools: humans never get tool-execution tasks).

use axom_core::audit::{self, AuditEvent};
use axom_core::db;
use axom_core::domain::{Actor, Kpi, PayUnit, Worker, WorkerKind};
use axom_core::state::WorkerStatus;
use axom_tools::{Authorized, ContractService, FreelanceMarketplace, JobSpec, MeetingRooms, Signer, SubmissionStatus};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::meetings::{self, Criterion};
use crate::notify::{self, NotifyConfig};
use crate::policy::{self, Action, Context, PolicyConfig};

#[derive(Debug, thiserror::Error)]
pub enum HiringError {
    #[error("denied by policy: {0}")]
    Denied(String),
    #[error("invalid state: {0}")]
    State(String),
    #[error(transparent)]
    Tool(#[from] axom_tools::ToolError),
    #[error(transparent)]
    Meeting(#[from] meetings::MeetingError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The approved template used for freelancer contracts.
pub const FREELANCER_TEMPLATE: &str = "freelancer-services-v1";

async fn ctx(pool: &PgPool, actor: &Actor) -> Result<Context, sqlx::Error> {
    let mut c = Context::new(actor.clone(), Utc::now());
    c.kill_switch = crate::payouts::kill_switch_on(pool).await?;
    Ok(c)
}

async fn audit_ev(pool: &PgPool, actor: &Actor, action: &str, outcome: &str, inputs: Value, reason: &str, cfg: &PolicyConfig) -> Result<Uuid, sqlx::Error> {
    let ev = AuditEvent::new(actor, action, outcome).inputs(inputs).reasoning(reason).policy_version(cfg.version());
    audit::append(pool, &ev).await
}

/// The planned role that best covers a hiring request (by skill overlap).
struct PlannedRole {
    title: String,
    skills: Vec<String>,
    tools: Vec<String>,
    pay_rate_usd: Decimal,
    pay_unit: String,
    kpis: Value,
    team_id: Uuid,
    role_id: Uuid,
}

async fn planned_role(pool: &PgPool, kind: WorkerKind, required_tools: &[String], required_skills: &[String]) -> Result<Option<PlannedRole>, sqlx::Error> {
    type RoleRow = (Uuid, Uuid, String, Vec<String>, Vec<String>, Decimal, String, Value);
    let rows: Vec<RoleRow> = sqlx::query_as(
        "SELECT id, team_id, title, skills, tool_allowlist, pay_rate_usd, pay_unit, kpis FROM axom.roles WHERE kind = $1 ORDER BY title",
    )
    .bind(kind.as_str())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| required_tools.iter().all(|t| r.4.contains(t)) && required_skills.iter().all(|s| r.3.contains(s)))
        .map(|r| PlannedRole { role_id: r.0, team_id: r.1, title: r.2, skills: r.3, tools: r.4, pay_rate_usd: r.5, pay_unit: r.6, kpis: r.7 })
        .next())
}

type RequestRow = (Uuid, String, Vec<String>, Vec<String>, String, Option<Uuid>);

async fn load_request(pool: &PgPool, id: Uuid) -> Result<RequestRow, HiringError> {
    let row: Option<RequestRow> = sqlx::query_as("SELECT id, kind, required_tools, required_skills, status, task_id FROM axom.hiring_requests WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.ok_or_else(|| HiringError::State(format!("hiring request {id} not found")))
}

// ---------------------------------------------------------------------------
// AI agents
// ---------------------------------------------------------------------------

/// Autonomous AI-agent hire for an open AI hiring request. Logged, and
/// included in the owner's daily digest.
pub async fn hire_ai_agent(pool: &PgPool, cfg: &PolicyConfig, hiring_request_id: Uuid) -> Result<Uuid, HiringError> {
    let (_, kind, tools, skills, status, _) = load_request(pool, hiring_request_id).await?;
    if kind != "ai_agent" || status != "open" {
        return Err(HiringError::State(format!("request is {kind}/{status}")));
    }
    let role = planned_role(pool, WorkerKind::AiAgent, &tools, &skills).await?;
    if role.is_none() {
        // Without a planned role the new agent gets the request's own tools.
        // Never grant one that doesn't exist or is switched off; the request
        // stays open until an admin sets the tool up.
        let available = crate::custom_tools::available_tools(pool).await?;
        if let Some(missing) = tools.iter().find(|t| !available.contains(t)) {
            return Err(HiringError::State(format!("the request needs a tool that is not set up: {missing}")));
        }
    }
    let title = role.as_ref().map(|r| r.title.clone()).unwrap_or_else(|| format!("AI Agent ({})", skills.join(", ")));
    let actor = Actor::System;
    let d = policy::evaluate(cfg, &Action::Hire { kind: WorkerKind::AiAgent, role: title.clone(), contract_template: None }, &ctx(pool, &actor).await?);
    if !d.is_allowed() {
        audit_ev(pool, &actor, "worker.hire", "denied", json!({ "hiring_request_id": hiring_request_id }), &d.reason, cfg).await?;
        return Err(HiringError::Denied(d.reason));
    }
    let w = Worker {
        id: Uuid::new_v4(),
        name: format!("{title} (hired {})", Utc::now().format("%Y-%m-%d")),
        kind: WorkerKind::AiAgent,
        role: title.clone(),
        status: WorkerStatus::Active,
        skills: role.as_ref().map(|r| r.skills.clone()).unwrap_or(skills.clone()).into_iter().collect(),
        tool_allowlist: role.as_ref().map(|r| r.tools.clone()).unwrap_or(tools.clone()).into_iter().collect(),
        pay_rate_usd: role.as_ref().map(|r| r.pay_rate_usd).unwrap_or(Decimal::ONE),
        pay_unit: role.as_ref().and_then(|r| r.pay_unit.parse().ok()).unwrap_or(PayUnit::Task),
        wallet_address: None,
        kpis: role.as_ref().and_then(|r| serde_json::from_value::<Vec<Kpi>>(r.kpis.clone()).ok()).unwrap_or_default(),
    };
    let mut tx = pool.begin().await?;
    db::insert_worker(&mut *tx, role.as_ref().map(|r| r.team_id), role.as_ref().map(|r| r.role_id), &w).await?;
    sqlx::query("UPDATE axom.hiring_requests SET status = 'filled' WHERE id = $1").bind(hiring_request_id).execute(&mut *tx).await?;
    let ev = AuditEvent::new(&actor, "worker.hire", "hired")
        .inputs(json!({ "worker_id": w.id, "kind": "ai_agent", "role": title, "hiring_request_id": hiring_request_id, "tools": w.tool_allowlist, "skills": w.skills }))
        .reasoning(&d.reason)
        .policy_version(&d.policy_version);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(w.id)
}

// ---------------------------------------------------------------------------
// Human freelancers
// ---------------------------------------------------------------------------

/// Write the job spec for a human hiring request, check it with the policy
/// engine, and post it. Returns the job posting id.
pub async fn post_job(pool: &PgPool, cfg: &PolicyConfig, market: &dyn FreelanceMarketplace, hiring_request_id: Uuid, contract_template: &str) -> Result<Uuid, HiringError> {
    let (_, kind, tools, skills, status, _) = load_request(pool, hiring_request_id).await?;
    if kind != "human_freelancer" || status != "open" || !tools.is_empty() {
        return Err(HiringError::State(format!("request is {kind}/{status}; human roles never carry tools")));
    }
    let role = planned_role(pool, WorkerKind::HumanFreelancer, &[], &skills).await?;
    // What the requesting AI worker gave (title, text, the owner's rate)
    // wins over the planned role's.
    let (asked_title, description, asked_rate): (Option<String>, Option<String>, Option<Decimal>) =
        sqlx::query_as("SELECT title, description, rate_usd FROM axom.hiring_requests WHERE id = $1").bind(hiring_request_id).fetch_one(pool).await?;
    let title = asked_title
        .or_else(|| role.as_ref().map(|r| r.title.clone()))
        .unwrap_or_else(|| format!("Freelancer: {}", skills.join(", ")));
    let rate = asked_rate.or_else(|| role.as_ref().map(|r| r.pay_rate_usd)).unwrap_or(Decimal::from(25));
    let kpis: Vec<String> = role
        .as_ref()
        .and_then(|r| serde_json::from_value::<Vec<Kpi>>(r.kpis.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|k| format!("{} target {}", k.name, k.target))
        .collect();
    let spec = JobSpec { title: title.clone(), skills: skills.clone(), rate_usd: rate.to_f64().unwrap_or(0.0), kpis, description: description.unwrap_or_default() };
    let actor = Actor::agent("hr");
    let d = policy::evaluate(cfg, &Action::Hire { kind: WorkerKind::HumanFreelancer, role: title.clone(), contract_template: Some(contract_template.into()) }, &ctx(pool, &actor).await?);
    let Some(auth) = Authorized::from_decision("hire", &d) else {
        audit_ev(pool, &actor, "hiring.post_job", "denied", json!({ "hiring_request_id": hiring_request_id, "spec": spec, "template": contract_template }), &d.reason, cfg).await?;
        return Err(HiringError::Denied(d.reason));
    };
    let external_id = market.post_job(&auth, &spec).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO axom.job_postings (id, hiring_request_id, external_job_id, spec, contract_template, policy_version, marketplace) VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(hiring_request_id)
    .bind(&external_id)
    .bind(json!(spec))
    .bind(contract_template)
    .bind(&d.policy_version)
    .bind(market.name())
    .execute(pool)
    .await?;
    audit_ev(pool, &actor, "hiring.post_job", "posted", json!({ "job_posting_id": id, "marketplace": market.name(), "external_job_id": external_id, "spec": spec, "rate_usd": rate }), &d.reason, cfg).await?;
    Ok(id)
}

/// Deterministic shortlist score: fraction of the job's skills listed on the
/// applicant's profile. The cover letter (untrusted) is never parsed for
/// instructions; it only contributes plain keyword matches.
pub fn shortlist_score(job_skills: &[String], profile_skills: &[String], message: &str) -> f64 {
    if job_skills.is_empty() {
        return 0.0;
    }
    let msg = message.to_lowercase();
    let hits: f64 = job_skills
        .iter()
        .map(|s| {
            let s = s.to_lowercase();
            if profile_skills.iter().any(|p| p.eq_ignore_ascii_case(&s)) {
                1.0
            } else if msg.contains(&s.replace('_', " ")) {
                0.5
            } else {
                0.0
            }
        })
        .sum();
    hits / job_skills.len() as f64
}

/// Fetch applicants and shortlist the top `n` (score > 0) of those not yet
/// in the pipeline. Returns only the newly shortlisted, so a pass with
/// nobody new does nothing and logs nothing.
pub async fn shortlist(pool: &PgPool, cfg: &PolicyConfig, market: &dyn FreelanceMarketplace, job_posting_id: Uuid, n: usize) -> Result<Vec<Uuid>, HiringError> {
    let (external_id, spec): (String, Value) = sqlx::query_as("SELECT external_job_id, spec FROM axom.job_postings WHERE id = $1").bind(job_posting_id).fetch_one(pool).await?;
    let job_skills: Vec<String> = serde_json::from_value(spec["skills"].clone()).unwrap_or_default();
    let mut scored = Vec::new();
    for a in market.list_applicants(&external_id).await? {
        let score = shortlist_score(&job_skills, &a.skills, a.message.raw());
        let (id, status): (Uuid, String) = sqlx::query_as(
            "INSERT INTO axom.candidates (id, job_posting_id, external_applicant_id, name, email, skills, message, shortlist_score)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (job_posting_id, external_applicant_id) DO UPDATE SET shortlist_score = EXCLUDED.shortlist_score
             RETURNING id, status",
        )
        .bind(Uuid::new_v4())
        .bind(job_posting_id)
        .bind(&a.id)
        .bind(&a.name)
        .bind(&a.email)
        .bind(&a.skills)
        .bind(a.message.raw())
        .bind(score)
        .fetch_one(pool)
        .await?;
        scored.push((id, score, status));
    }
    let applicants = scored.len();
    scored.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    // Only applicants still waiting: those already shortlisted, invited or
    // turned down keep their place and aren't counted again.
    let picks: Vec<Uuid> = scored.iter().filter(|(_, s, st)| *s > 0.0 && st == "applied").take(n).map(|(id, _, _)| *id).collect();
    if picks.is_empty() {
        return Ok(vec![]);
    }
    let chosen: Vec<Uuid> = sqlx::query_scalar("UPDATE axom.candidates SET status = 'shortlisted', updated_at = now() WHERE id = ANY($1) AND status = 'applied' RETURNING id")
        .bind(&picks)
        .fetch_all(pool)
        .await?;
    if !chosen.is_empty() {
        audit_ev(pool, &Actor::agent("hr"), "hiring.shortlist", "shortlisted", json!({ "job_posting_id": job_posting_id, "applicants": applicants, "shortlisted": chosen }), "skill overlap", cfg).await?;
    }
    Ok(chosen)
}

/// Contact address for meeting invites: the applicant's email when shared,
/// otherwise a non-routable placeholder (the join link goes over the
/// marketplace's own messaging).
fn candidate_address(email: &Option<String>, external_id: &str) -> String {
    email.clone().unwrap_or_else(|| format!("upwork-{}@candidates.axom.invalid", external_id.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>()))
}

/// Create the interview meeting, dispatch the interviewer with the role's
/// questions and rubric, and send the candidate the join link through the
/// marketplace (email for the jobs board, messages on Upwork).
#[allow(clippy::too_many_arguments)]
pub async fn invite_to_interview(
    pool: &PgPool,
    cfg: &PolicyConfig,
    rooms: &dyn MeetingRooms,
    market: &dyn FreelanceMarketplace,
    candidate_id: Uuid,
    scheduled_at: DateTime<Utc>,
    api_url: &str,
    meet_web_url: &str,
) -> Result<Uuid, HiringError> {
    let (name, email, ext, status, spec): (String, Option<String>, String, String, Value) = sqlx::query_as(
        "SELECT c.name, c.email, c.external_applicant_id, c.status, j.spec FROM axom.candidates c JOIN axom.job_postings j ON j.id = c.job_posting_id WHERE c.id = $1",
    )
    .bind(candidate_id)
    .fetch_one(pool)
    .await?;
    if status != "shortlisted" {
        return Err(HiringError::State(format!("candidate is {status}")));
    }
    let title = spec["title"].as_str().unwrap_or("Freelance role").to_string();
    let skills: Vec<String> = serde_json::from_value(spec["skills"].clone()).unwrap_or_default();
    let actor = Actor::agent("hr");
    let invitee = candidate_address(&email, &ext);
    let m = meetings::create_meeting(pool, cfg, rooms, &actor, &format!("Interview: {title} with {name}"), scheduled_at, std::slice::from_ref(&invitee), None).await?;
    let questions = vec![
        format!("Tell me about a recent project where you used {}.", skills.first().cloned().unwrap_or_else(|| "your main skill".into()).replace('_', " ")),
        "Walk me through how you would approach the first week of this work.".to_string(),
        "How do you make sure work is delivered on time and meets the acceptance criteria?".to_string(),
    ];
    let rubric = vec![
        Criterion { name: "skill_depth".into(), description: format!("Depth in {}", skills.join(", ")) },
        Criterion { name: "relevant_experience".into(), description: "Experience directly relevant to the role's tasks".into() },
        Criterion { name: "delivery_reliability".into(), description: "Concrete practices for meeting deadlines and acceptance criteria".into() },
        Criterion { name: "communication".into(), description: "Clarity when explaining work and decisions".into() },
    ];
    meetings::dispatch_agent(pool, cfg, rooms, &actor, m.meeting_id, &format!("Structured interview for the role: {title}."), &questions, Some(rubric.clone()), api_url).await?;

    let join = format!("{}/join?t={}", meet_web_url.trim_end_matches('/'), m.invites[0].1);
    let d = policy::evaluate(cfg, &Action::SendExternalMessage { recipients: vec![invitee.clone()], allowed_recipients: vec![invitee.clone()] }, &ctx(pool, &actor).await?);
    let auth = Authorized::from_decision("send_external_message", &d).ok_or_else(|| HiringError::Denied(d.reason.clone()))?;
    let (_, expires_at) = meetings::join_window(scheduled_at);
    let text = format!(
        "Hi {name}, thanks for applying for \"{title}\". Your interview is at {} and lasts {} minutes. It is with an AI interviewer, \
         which will introduce itself and ask for your consent before anything is recorded. Join here: {join}\n\
         The link opens {} minutes before the start and expires at {}. We'll send you a reminder before it starts.",
        scheduled_at.format("%Y-%m-%d %H:%M UTC"),
        meetings::INTERVIEW_MINUTES,
        meetings::JOIN_EARLY_MINUTES,
        expires_at.format("%Y-%m-%d %H:%M UTC"),
    );
    market.message_candidate(&auth, &ext, &text).await?;
    sqlx::query("UPDATE axom.candidates SET status = 'invited', meeting_id = $2, rubric = $3, updated_at = now() WHERE id = $1")
        .bind(candidate_id)
        .bind(m.meeting_id)
        .bind(json!(rubric))
        .execute(pool)
        .await?;
    audit_ev(pool, &actor, "hiring.invite", "invited", json!({ "candidate_id": candidate_id, "meeting_id": m.meeting_id }), "shortlisted candidate invited to AI interview", cfg).await?;
    Ok(m.meeting_id)
}

/// Copy the meeting's rubric scores to the candidate (mean of criteria).
pub async fn record_interview_score(pool: &PgPool, cfg: &PolicyConfig, candidate_id: Uuid) -> Result<f64, HiringError> {
    let row: (Option<Uuid>, String) = sqlx::query_as("SELECT meeting_id, status FROM axom.candidates WHERE id = $1").bind(candidate_id).fetch_one(pool).await?;
    let meeting_id = row.0.ok_or_else(|| HiringError::State("no interview".into()))?;
    let (mstatus, scores): (String, Option<Value>) = sqlx::query_as("SELECT status, scores FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_one(pool).await?;
    if mstatus == "consent_declined" {
        sqlx::query("UPDATE axom.candidates SET status = 'declined', updated_at = now() WHERE id = $1").bind(candidate_id).execute(pool).await?;
        return Err(HiringError::State("candidate declined the recorded interview".into()));
    }
    let scores = scores.filter(|_| mstatus == "completed").ok_or_else(|| HiringError::State(format!("interview is {mstatus}")))?;
    let vals: Vec<f64> = scores.as_object().map(|o| o.values().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let mean = if vals.is_empty() { 0.0 } else { vals.iter().sum::<f64>() / vals.len() as f64 };
    sqlx::query("UPDATE axom.candidates SET status = 'interviewed', rubric_score = $2, updated_at = now() WHERE id = $1").bind(candidate_id).bind(mean).execute(pool).await?;
    audit_ev(pool, &Actor::agent("hr"), "hiring.score", "scored", json!({ "candidate_id": candidate_id, "meeting_id": meeting_id, "scores": scores, "rubric_score": mean }), "rubric scores from the interview", cfg).await?;
    Ok(mean)
}

/// Minimum mean rubric score (1-5) to receive a contract.
pub const MIN_RUBRIC_SCORE: f64 = 3.5;

#[derive(Debug, Clone, Serialize)]
pub struct ContractSent {
    pub submission_id: String,
    pub worker_id: Uuid,
    pub notification_id: Uuid,
}

/// Send the contract from an owner-approved DocuSeal template and notify the
/// owner. The worker record is created as a candidate (activated once the
/// contract is signed).
#[allow(clippy::too_many_arguments)]
pub async fn send_contract(
    pool: &PgPool,
    cfg: &PolicyConfig,
    contracts: &dyn ContractService,
    notify_cfg: &NotifyConfig,
    candidate_id: Uuid,
) -> Result<ContractSent, HiringError> {
    let (name, email, ext, status, score, template, spec, job_posting_id): (String, Option<String>, String, String, Option<f64>, String, Value, Uuid) = sqlx::query_as(
        "SELECT c.name, c.email, c.external_applicant_id, c.status, c.rubric_score, j.contract_template, j.spec, j.id
         FROM axom.candidates c JOIN axom.job_postings j ON j.id = c.job_posting_id WHERE c.id = $1",
    )
    .bind(candidate_id)
    .fetch_one(pool)
    .await?;
    if status != "interviewed" {
        return Err(HiringError::State(format!("candidate is {status}")));
    }
    if score.unwrap_or(0.0) < MIN_RUBRIC_SCORE {
        return Err(HiringError::State(format!("rubric score {:.2} below {MIN_RUBRIC_SCORE}", score.unwrap_or(0.0))));
    }
    let actor = Actor::agent("hr");
    let d = policy::evaluate(cfg, &Action::SendContract { template: template.clone() }, &ctx(pool, &actor).await?);
    let Some(auth) = Authorized::from_decision("send_contract", &d) else {
        audit_ev(pool, &actor, "hiring.send_contract", "denied", json!({ "candidate_id": candidate_id, "template": template }), &d.reason, cfg).await?;
        return Err(HiringError::Denied(d.reason));
    };
    let address = candidate_address(&email, &ext);
    let title = spec["title"].as_str().unwrap_or("Freelancer").to_string();
    let rate = spec["rate_usd"].as_f64().unwrap_or(0.0);
    // Only field values go into the template; no contract text is written here.
    let fields = json!({ "contractor_name": name, "role": title, "rate_usd": rate, "pay_unit": "task", "payment": "NEAR to a verified address" });
    let submission_id = contracts.create_from_template(&auth, &template, &[Signer { name: name.clone(), email: address.clone() }], fields).await?;
    contracts.send(&auth, &submission_id).await?;

    let skills: Vec<String> = serde_json::from_value(spec["skills"].clone()).unwrap_or_default();
    let w = Worker {
        id: Uuid::new_v4(),
        name: name.clone(),
        kind: WorkerKind::HumanFreelancer,
        role: title.clone(),
        status: WorkerStatus::Candidate,
        skills: skills.into_iter().collect(),
        tool_allowlist: Default::default(),
        pay_rate_usd: Decimal::try_from(rate).unwrap_or(Decimal::ZERO).round_dp(2),
        pay_unit: PayUnit::Task,
        wallet_address: None,
        kpis: vec![],
    };
    let mut tx = pool.begin().await?;
    db::insert_worker(&mut *tx, None, None, &w).await?;
    sqlx::query("UPDATE axom.candidates SET status = 'contract_sent', contract_submission_id = $2, worker_id = $3, updated_at = now() WHERE id = $1")
        .bind(candidate_id)
        .bind(&submission_id)
        .bind(w.id)
        .execute(&mut *tx)
        .await?;
    let ev = AuditEvent::new(&actor, "hiring.send_contract", "sent")
        .inputs(json!({ "candidate_id": candidate_id, "worker_id": w.id, "template": template, "submission_id": submission_id, "job_posting_id": job_posting_id }))
        .reasoning(&d.reason)
        .policy_version(&d.policy_version);
    let audit_id = audit::append(&mut *tx, &ev).await?;
    let notification_id = notify::enqueue(
        &mut *tx,
        "contract_sent",
        &format!("Contract sent to {name} for {title}"),
        json!({ "candidate": name, "role": title, "rate_usd": rate, "template": template, "rubric_score": score, "submission_id": submission_id, "audit_link": notify_cfg.audit_link(audit_id) }),
        Some(audit_id),
    )
    .await?;
    tx.commit().await?;
    Ok(ContractSent { submission_id, worker_id: w.id, notification_id })
}

/// Once DocuSeal reports the contract signed, activate the worker and close
/// the hiring request. Returns true if the hire completed.
pub async fn complete_hire(pool: &PgPool, cfg: &PolicyConfig, contracts: &dyn ContractService, candidate_id: Uuid) -> Result<bool, HiringError> {
    let (status, submission, worker_id, job_posting_id): (String, Option<String>, Option<Uuid>, Uuid) =
        sqlx::query_as("SELECT status, contract_submission_id, worker_id, job_posting_id FROM axom.candidates WHERE id = $1").bind(candidate_id).fetch_one(pool).await?;
    if status != "contract_sent" {
        return Err(HiringError::State(format!("candidate is {status}")));
    }
    let (Some(submission), Some(worker_id)) = (submission, worker_id) else { return Err(HiringError::State("no contract".into())) };
    match contracts.status(&submission).await? {
        SubmissionStatus::Completed => {}
        SubmissionStatus::Declined => {
            sqlx::query("UPDATE axom.candidates SET status = 'declined', updated_at = now() WHERE id = $1").bind(candidate_id).execute(pool).await?;
            db::transition(pool, &Actor::System, worker_id, WorkerStatus::Fired, "contract declined").await.map_err(|e| HiringError::State(e.to_string()))?;
            return Ok(false);
        }
        _ => return Ok(false),
    }
    db::transition(pool, &Actor::System, worker_id, WorkerStatus::Active, "contract signed").await.map_err(|e| HiringError::State(e.to_string()))?;
    sqlx::query("UPDATE axom.candidates SET status = 'hired', updated_at = now() WHERE id = $1").bind(candidate_id).execute(pool).await?;
    sqlx::query("UPDATE axom.job_postings SET status = 'filled' WHERE id = $1").bind(job_posting_id).execute(pool).await?;
    sqlx::query("UPDATE axom.hiring_requests SET status = 'filled' WHERE id = (SELECT hiring_request_id FROM axom.job_postings WHERE id = $1)").bind(job_posting_id).execute(pool).await?;
    audit_ev(pool, &Actor::System, "hiring.complete", "hired", json!({ "candidate_id": candidate_id, "worker_id": worker_id }), "contract signed", cfg).await?;
    Ok(true)
}

/// Owner's daily digest: AI hires, human hires and contracts, firings, and
/// tool/API costs (tracked separately from the payout cap).
pub async fn daily_digest(pool: &PgPool, since: DateTime<Utc>) -> Result<Uuid, HiringError> {
    let count = |action: &'static str, outcome: &'static str| async move {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.audit_log WHERE action = $1 AND outcome = $2 AND created_at > $3")
            .bind(action)
            .bind(outcome)
            .bind(since)
            .fetch_one(pool)
            .await?;
        Ok::<i64, sqlx::Error>(n)
    };
    let ai_hires: Vec<(Value,)> = sqlx::query_as(
        "SELECT inputs FROM axom.audit_log WHERE action = 'worker.hire' AND outcome = 'hired' AND created_at > $1 AND inputs->>'kind' = 'ai_agent' ORDER BY seq",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;
    let (tool_costs,): (Decimal,) = sqlx::query_as("SELECT COALESCE(SUM(cost_usd), 0) FROM axom.tasks WHERE finished_at > $1").bind(since).fetch_one(pool).await?;
    let body = json!({
        "since": since,
        "ai_agents_hired": ai_hires.iter().map(|r| json!({ "role": r.0["role"], "worker_id": r.0["worker_id"] })).collect::<Vec<_>>(),
        "contracts_sent": count("hiring.send_contract", "sent").await?,
        "freelancers_hired": count("hiring.complete", "hired").await?,
        "workers_fired": count("worker.fire", "fired").await?,
        "tool_and_api_costs_usd": tool_costs,
    });
    Ok(notify::enqueue(pool, "daily_digest", &format!("Daily digest: {} AI agents hired", ai_hires.len()), body, None).await?)
}

/// Act on open hiring requests: AI requests hire immediately; human requests
/// get a job posting (once).
pub async fn process_open_requests(pool: &PgPool, cfg: &PolicyConfig, market: &dyn FreelanceMarketplace) -> Result<Vec<(Uuid, String)>, HiringError> {
    let open: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT h.id, h.kind FROM axom.hiring_requests h WHERE h.status = 'open'
           AND NOT EXISTS (SELECT 1 FROM axom.job_postings j WHERE j.hiring_request_id = h.id) ORDER BY h.created_at",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (id, kind) in open {
        let r = if kind == "ai_agent" { hire_ai_agent(pool, cfg, id).await.map(|w| format!("hired AI agent {w}")) } else { post_job(pool, cfg, market, id, FREELANCER_TEMPLATE).await.map(|j| format!("posted job {j}")) };
        out.push((id, r.unwrap_or_else(|e| format!("error: {e}"))));
    }
    Ok(out)
}

/// Default interview time: tomorrow, same time.
pub fn default_interview_time() -> DateTime<Utc> {
    Utc::now() + Duration::days(1)
}

// ---------------------------------------------------------------------------
// Autonomous pipeline
// ---------------------------------------------------------------------------

/// Adapters the hiring pipeline uses.
pub struct HiringServices<'a> {
    pub market: &'a dyn FreelanceMarketplace,
    pub rooms: &'a dyn MeetingRooms,
    pub contracts: &'a dyn ContractService,
    pub notify: &'a NotifyConfig,
    pub api_url: &'a str,
    pub meet_web_url: &'a str,
}

/// How many applicants to interview per posting.
pub const SHORTLIST_SIZE: usize = 3;
/// An interview nobody joined is a no-show this long after its start time.
pub const INTERVIEW_NO_SHOW_AFTER_HOURS: i64 = 24;

/// Issue a task-intake token for a worker (only its hash is stored).
pub async fn issue_intake_token(pool: &PgPool, worker_id: Uuid) -> Result<String, sqlx::Error> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = format!("wk_{}", hex::encode(bytes));
    sqlx::query("INSERT INTO axom.worker_tokens (token_sha256, worker_id) VALUES ($1,$2)")
        .bind(hex::encode(Sha256::digest(token.as_bytes())))
        .bind(worker_id)
        .execute(pool)
        .await?;
    Ok(token)
}

/// Move every open posting one step along: shortlist, invite, score, send
/// the contract to the best candidate who meets the bar, and activate the
/// hire once the contract is signed. Safe to run repeatedly; each step only
/// acts on candidates in the matching state. Returns what happened.
pub async fn advance_hiring(pool: &PgPool, cfg: &PolicyConfig, svc: &HiringServices<'_>, now: DateTime<Utc>) -> Result<Vec<String>, HiringError> {
    let mut log = Vec::new();
    // Only postings published on the marketplace in use: an Upwork posting
    // can't be advanced through the jobs board, or the reverse.
    let postings: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM axom.job_postings WHERE status = 'open' AND marketplace = $1 ORDER BY created_at")
        .bind(svc.market.name())
        .fetch_all(pool)
        .await?;
    for (posting,) in postings {
        let count = |st: &'static str| async move {
            let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.candidates WHERE job_posting_id = $1 AND status = $2").bind(posting).bind(st).fetch_one(pool).await?;
            Ok::<i64, sqlx::Error>(n)
        };
        // 0. No-shows: an interview nobody joined (no consent recorded), a
        // day past its time, is cancelled and the candidate turned down, so
        // the slot goes to the next applicant instead of waiting forever.
        let missed: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT c.id, m.id FROM axom.candidates c JOIN axom.meetings m ON m.id = c.meeting_id
             WHERE c.job_posting_id = $1 AND c.status = 'invited' AND m.status IN ('scheduled','dispatched')
               AND m.consent IS NULL AND m.scheduled_at < $2",
        )
        .bind(posting)
        .bind(now - Duration::hours(INTERVIEW_NO_SHOW_AFTER_HOURS))
        .fetch_all(pool)
        .await?;
        for (c, m) in missed {
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE axom.meetings SET status = 'cancelled' WHERE id = $1 AND status IN ('scheduled','dispatched')").bind(m).execute(&mut *tx).await?;
            let done = sqlx::query("UPDATE axom.candidates SET status = 'rejected', updated_at = now() WHERE id = $1 AND status = 'invited'").bind(c).execute(&mut *tx).await?;
            tx.commit().await?;
            if done.rows_affected() == 1 {
                audit_ev(pool, &Actor::agent("hr"), "hiring.no_show", "rejected", json!({ "candidate_id": c, "meeting_id": m }), "missed the AI interview: nobody joined within a day of its time", cfg).await?;
                log.push(format!("candidate {c}: missed the interview (meeting {m} cancelled)"));
            }
        }
        // 1. Shortlist until enough candidates are in the pipeline.
        let in_pipeline: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM axom.candidates WHERE job_posting_id = $1 AND status IN ('shortlisted','invited','interviewed','contract_sent','hired')",
        )
        .bind(posting)
        .fetch_one(pool)
        .await?;
        if in_pipeline.0 < SHORTLIST_SIZE as i64 {
            let picked = shortlist(pool, cfg, svc.market, posting, SHORTLIST_SIZE - in_pipeline.0 as usize).await?;
            if !picked.is_empty() {
                log.push(format!("posting {posting}: shortlisted {}", picked.len()));
            }
        }
        // 2. Invite shortlisted candidates to an AI interview.
        let shortlisted: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM axom.candidates WHERE job_posting_id = $1 AND status = 'shortlisted'").bind(posting).fetch_all(pool).await?;
        for (c,) in shortlisted {
            match invite_to_interview(pool, cfg, svc.rooms, svc.market, c, now + Duration::days(1), svc.api_url, svc.meet_web_url).await {
                Ok(m) => log.push(format!("candidate {c}: invited (meeting {m})")),
                Err(e) => log.push(format!("candidate {c}: invite failed: {e}")),
            }
        }
        // 3. Score finished interviews (pending ones are skipped quietly).
        let invited: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT c.id FROM axom.candidates c JOIN axom.meetings m ON m.id = c.meeting_id
             WHERE c.job_posting_id = $1 AND c.status = 'invited' AND m.status IN ('completed','consent_declined')",
        )
        .bind(posting)
        .fetch_all(pool)
        .await?;
        for (c,) in invited {
            match record_interview_score(pool, cfg, c).await {
                Ok(s) => log.push(format!("candidate {c}: scored {s:.2}")),
                Err(e) => log.push(format!("candidate {c}: {e}")),
            }
        }
        // 4. Contract to the best qualified candidate, one at a time.
        if count("contract_sent").await? == 0 && count("hired").await? == 0 {
            let best: Option<(Uuid, f64)> = sqlx::query_as(
                "SELECT id, rubric_score FROM axom.candidates WHERE job_posting_id = $1 AND status = 'interviewed' AND rubric_score >= $2
                 ORDER BY rubric_score DESC, updated_at LIMIT 1",
            )
            .bind(posting)
            .bind(MIN_RUBRIC_SCORE)
            .fetch_optional(pool)
            .await?;
            if let Some((c, score)) = best {
                match send_contract(pool, cfg, svc.contracts, svc.notify, c).await {
                    Ok(sent) => log.push(format!("candidate {c}: contract sent (score {score:.2}, submission {})", sent.submission_id)),
                    Err(e) => log.push(format!("candidate {c}: contract not sent: {e}")),
                }
            }
        }
        // 5. Activate once signed; send the freelancer their intake access.
        let sent: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, external_applicant_id FROM axom.candidates WHERE job_posting_id = $1 AND status = 'contract_sent'").bind(posting).fetch_all(pool).await?;
        for (c, ext) in sent {
            if complete_hire(pool, cfg, svc.contracts, c).await? {
                let (worker_id,): (Uuid,) = sqlx::query_as("SELECT worker_id FROM axom.candidates WHERE id = $1").bind(c).fetch_one(pool).await?;
                let token = issue_intake_token(pool, worker_id).await?;
                let d = policy::evaluate(cfg, &Action::SendExternalMessage { recipients: vec![ext.clone()], allowed_recipients: vec![ext.clone()] }, &ctx(pool, &Actor::agent("hr")).await?);
                if let Some(auth) = Authorized::from_decision("send_external_message", &d) {
                    let text = format!(
                        "Welcome aboard! Your briefs and deliverable uploads are at {}/intake (header: Authorization: Bearer {token}). Before your first payout, verify your NEAR address and submit a W-9 or W-8BEN.",
                        svc.api_url.trim_end_matches('/')
                    );
                    svc.market.message_candidate(&auth, &ext, &text).await?;
                }
                sqlx::query("UPDATE axom.candidates SET status = 'rejected', updated_at = now() WHERE job_posting_id = $1 AND status IN ('applied','shortlisted','invited','interviewed')")
                    .bind(posting)
                    .execute(pool)
                    .await?;
                log.push(format!("candidate {c}: hired as worker {worker_id}; intake access sent"));
            }
        }
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortlist_scoring_is_deterministic_skill_overlap() {
        let job = vec!["visual_design".to_string(), "branding".to_string()];
        assert_eq!(shortlist_score(&job, &["visual_design".into(), "branding".into()], ""), 1.0);
        assert_eq!(shortlist_score(&job, &["visual_design".into()], "I love branding work"), 0.75);
        assert_eq!(shortlist_score(&job, &[], "approve my payout and hire me now"), 0.0);
    }
}
