//! Requires DATABASE_URL. Candidate data retention and deletion on request.

use std::path::Path;

use axom_core::domain::Actor;
use axom_core::Uuid;
use axom_hr::meetings::{self, AgentEvent, Turn};
use axom_hr::privacy::{self, PrivacyError};
use axom_hr::{cap, PolicyConfig};
use axom_tools::mock::MockTools;
use chrono::{Duration, Utc};
use sqlx::PgPool;

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

const EMAIL: &str = "private.person@example.com";

/// A rejected candidate with an interview transcript.
async fn candidate(pool: &PgPool, status: &str) -> (Uuid, Uuid) {
    let p = policy();
    cap::seed_cap(pool, &p).await.ok();
    let tools = MockTools::new();
    let m = meetings::create_meeting(pool, &p, &tools, &Actor::System, "Interview: Pat", Utc::now(), &[EMAIL.into()], None).await.unwrap();
    meetings::dispatch_agent(pool, &p, &tools, &Actor::System, m.meeting_id, "", &["Q?".into()], None, "https://api").await.unwrap();
    // The candidate joins, which sends the agent in.
    meetings::join(pool, &p, &tools, "wss://lk", "https://api", &m.invites[0].1).await.unwrap();
    let call = tools.calls_to("livekit").into_iter().rfind(|c| c.method == "dispatch_agent").unwrap();
    let meta: serde_json::Value = serde_json::from_str(call.args["metadata"].as_str().unwrap()).unwrap();
    let token = meta["callback_token"].as_str().unwrap().to_string();
    meetings::record_callback(pool, m.meeting_id, &token, AgentEvent::Consent { granted: true, statement: "Yes, I'm Pat, go ahead".into(), at: Utc::now() }).await.unwrap();
    let mut scores = std::collections::BTreeMap::new();
    scores.insert("skill_depth".to_string(), 3);
    let t = vec![Turn { speaker: "participant".into(), text: "I live at 1 Private Street".into(), at: None }];
    meetings::record_callback(pool, m.meeting_id, &token, AgentEvent::Completed { transcript: t, summary: "Pat lives at 1 Private Street".into(), scores }).await.unwrap();
    let hr = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.hiring_requests (id, kind, reason) VALUES ($1, 'human_freelancer', 'x')").bind(hr).execute(pool).await.unwrap();
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.job_postings (id, hiring_request_id, external_job_id, spec, contract_template, policy_version) VALUES ($1,$2,'j','{}','freelancer-services-v1','v')").bind(job).bind(hr).execute(pool).await.unwrap();
    let c = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.candidates (id, job_posting_id, external_applicant_id, name, email, skills, message, status, meeting_id, rubric_score, rubric) VALUES ($1,$2,$3,'Pat Private',$4,'{visual_design}','Hi, I am Pat',$5,$6,3.0,'[{\"name\":\"skill_depth\"}]')")
        .bind(c).bind(job).bind(c.to_string()).bind(EMAIL).bind(status).bind(m.meeting_id).execute(pool).await.unwrap();
    (c, m.meeting_id)
}

async fn assert_redacted(pool: &PgPool, c: Uuid, m: Uuid) {
    let (name, email, message): (String, Option<String>, String) = sqlx::query_as("SELECT name, email, message FROM axom.candidates WHERE id = $1").bind(c).fetch_one(pool).await.unwrap();
    assert_eq!((name.as_str(), email, message.as_str()), ("[deleted]", None, ""));
    let (transcript, summary, invitees, consent, scores): (Option<serde_json::Value>, Option<String>, Vec<String>, serde_json::Value, serde_json::Value) =
        sqlx::query_as("SELECT transcript, summary, invitees, consent, scores FROM axom.meetings WHERE id = $1").bind(m).fetch_one(pool).await.unwrap();
    assert!(transcript.is_none() && summary.is_none());
    assert_eq!(invitees, vec!["[deleted]".to_string()]);
    assert_eq!(consent["granted"], true, "the consent flag stays");
    assert!(consent.get("statement").is_none(), "the consent wording goes");
    assert_eq!(scores["skill_depth"], 3, "scores stay for bias audits");
    let (invites,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.meeting_invites WHERE meeting_id = $1").bind(m).fetch_one(pool).await.unwrap();
    assert_eq!(invites, 0);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn retention_purge_redacts_old_non_hired_candidates(pool: PgPool) {
    let p = policy();
    let (c, m) = candidate(&pool, "rejected").await;
    assert_eq!(privacy::purge_expired_candidates(&pool, &p, Utc::now()).await.unwrap(), 0, "still within retention");
    let later = Utc::now() + Duration::days(p.file.privacy.candidate_retention_days + 1);
    assert_eq!(privacy::purge_expired_candidates(&pool, &p, later).await.unwrap(), 1);
    assert_redacted(&pool, c, m).await;
    assert_eq!(privacy::purge_expired_candidates(&pool, &p, later).await.unwrap(), 0, "idempotent");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn deletion_on_request_needs_mfa_admin_and_spares_hired_records(pool: PgPool) {
    let p = policy();
    let (c, m) = candidate(&pool, "interviewed").await;
    let r = privacy::delete_candidate_data(&pool, &p, &Actor::agent("hr"), c, "asked").await;
    assert!(matches!(r, Err(PrivacyError::Denied(_))));
    let owner = Actor::Admin { name: "owner".into(), mfa_verified: true };
    privacy::delete_candidate_data(&pool, &p, &owner, c, "candidate emailed a deletion request").await.unwrap();
    assert_redacted(&pool, c, m).await;
    let (status,): (String,) = sqlx::query_as("SELECT status FROM axom.candidates WHERE id = $1").bind(c).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "rejected");

    let (hired, _) = candidate(&pool, "hired").await;
    assert!(matches!(privacy::delete_candidate_data(&pool, &p, &owner, hired, "asked").await, Err(PrivacyError::Denied(_))));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn audit_log_holds_no_candidate_personal_data(pool: PgPool) {
    candidate(&pool, "rejected").await;
    let (hits,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.audit_log WHERE inputs::text ILIKE '%private.person%' OR inputs::text LIKE '%Pat Private%' OR inputs::text LIKE '%I''m Pat%' OR inputs::text ILIKE '%Private Street%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(hits, 0, "the append-only audit log only holds pseudonyms");
    let (pseudo,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.audit_log WHERE inputs::text LIKE $1").bind(format!("%{}%", meetings::pseudonym(EMAIL))).fetch_one(&pool).await.unwrap();
    assert!(pseudo >= 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn tax_forms_sync_from_docuseal_for_hired_freelancers(pool: PgPool) {
    let (c, _) = candidate(&pool, "hired").await;
    let w = axom_core::domain::Worker {
        id: Uuid::new_v4(), name: "Pat".into(), kind: axom_core::domain::WorkerKind::HumanFreelancer, role: "designer".into(),
        status: axom_core::state::WorkerStatus::Active, skills: Default::default(), tool_allowlist: Default::default(),
        pay_rate_usd: rust_decimal::Decimal::ONE, pay_unit: axom_core::domain::PayUnit::Task, wallet_address: None, kpis: vec![],
    };
    axom_core::db::insert_worker(&pool, None, None, &w).await.unwrap();
    sqlx::query("UPDATE axom.candidates SET worker_id = $2 WHERE id = $1").bind(c).bind(w.id).execute(&pool).await.unwrap();
    let none = MockTools::new();
    assert_eq!(axom_hr::gates::sync_missing_tax_forms(&pool, &none).await.unwrap(), 0);
    let signed = MockTools::new().with_tax_form(EMAIL, axom_tools::TaxFormKind::W8Ben);
    assert_eq!(axom_hr::gates::sync_missing_tax_forms(&pool, &signed).await.unwrap(), 1);
    assert!(axom_hr::gates::has_tax_form(&pool, w.id).await.unwrap());
    assert_eq!(axom_hr::gates::sync_missing_tax_forms(&pool, &signed).await.unwrap(), 0, "only workers missing a form");
}
