//! Candidate data retention and deletion on request.
//!
//! Candidates who were not hired keep their data for the configured period
//! (`[privacy] candidate_retention_days`), then it is redacted: name, email,
//! profile skills, cover letter, interview transcript and summary, invitee
//! addresses, consent wording, and join links. The rubric, scores, and
//! consent flag stay without identity, so a bias audit remains possible.
//! The audit log only ever holds pseudonymous references (see
//! `meetings::pseudonym`), so nothing personal is left behind in it.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::policy::PolicyConfig;

#[derive(Debug, thiserror::Error)]
pub enum PrivacyError {
    #[error("not allowed: {0}")]
    Denied(String),
    #[error("candidate {0} not found")]
    NotFound(Uuid),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Statuses whose data is kept for payroll and tax records instead.
const HIRED: &[&str] = &["contract_sent", "hired"];

async fn redact(tx: &mut Transaction<'_, Postgres>, candidate_id: Uuid) -> Result<(), sqlx::Error> {
    let meeting: Option<(Option<Uuid>,)> = sqlx::query_as("SELECT meeting_id FROM axom.candidates WHERE id = $1").bind(candidate_id).fetch_optional(&mut **tx).await?;
    // The jobs-board application behind the candidate, if any.
    sqlx::query(
        "UPDATE axom.board_applications b SET name = '[deleted]', email = 'deleted-' || b.id || '@deleted.invalid', skills = '{}', message = '', portfolio_url = NULL
         FROM axom.candidates c JOIN axom.job_postings j ON j.id = c.job_posting_id
         WHERE c.id = $1 AND j.marketplace = 'axom_board' AND b.id::text = c.external_applicant_id",
    )
    .bind(candidate_id)
    .execute(&mut **tx)
    .await?;
    // ...and the CV uploaded with it.
    sqlx::query(
        "DELETE FROM axom.board_application_cvs f USING axom.candidates c JOIN axom.job_postings j ON j.id = c.job_posting_id
         WHERE c.id = $1 AND j.marketplace = 'axom_board' AND f.application_id::text = c.external_applicant_id",
    )
    .bind(candidate_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE axom.candidates SET name = '[deleted]', email = NULL, skills = '{}', message = '',
                status = CASE WHEN status IN ('applied','shortlisted','invited','interviewed') THEN 'rejected' ELSE status END,
                updated_at = now() WHERE id = $1",
    )
    .bind(candidate_id)
    .execute(&mut **tx)
    .await?;
    if let Some((Some(m),)) = meeting {
        sqlx::query(
            "UPDATE axom.meetings SET transcript = NULL, summary = NULL, invitees = ARRAY['[deleted]'], extra_recipients = '{}',
                    title = 'Interview (candidate data deleted)',
                    consent = CASE WHEN consent IS NULL THEN NULL ELSE jsonb_build_object('granted', consent->'granted', 'at', consent->'at') END
             WHERE id = $1",
        )
        .bind(m)
        .execute(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM axom.meeting_invites WHERE meeting_id = $1").bind(m).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Redact data of non-hired candidates older than the retention period.
pub async fn purge_expired_candidates(pool: &PgPool, cfg: &PolicyConfig, now: DateTime<Utc>) -> Result<usize, PrivacyError> {
    let cutoff = now - Duration::days(cfg.file.privacy.candidate_retention_days);
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM axom.candidates WHERE updated_at < $1 AND name <> '[deleted]' AND NOT (status = ANY($2))",
    )
    .bind(cutoff)
    .bind(HIRED)
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    for (id,) in &ids {
        redact(&mut tx, *id).await?;
    }
    // Jobs-board applications that never became candidates (the job closed
    // before shortlisting) are deleted outright.
    sqlx::query(
        "DELETE FROM axom.board_applications b WHERE b.created_at < $1
           AND NOT EXISTS (SELECT 1 FROM axom.candidates c WHERE c.external_applicant_id = b.id::text)",
    )
    .bind(cutoff)
    .execute(&mut *tx)
    .await?;
    if !ids.is_empty() {
        let ev = AuditEvent::new(&Actor::System, "privacy.retention_purge", "redacted")
            .inputs(json!({ "candidates": ids.iter().map(|r| r.0).collect::<Vec<_>>(), "retention_days": cfg.file.privacy.candidate_retention_days }))
            .policy_version(cfg.version());
        audit::append(&mut *tx, &ev).await?;
    }
    tx.commit().await?;
    Ok(ids.len())
}

/// Delete a candidate's data on request (owner, MFA-verified). Hired
/// workers' records are kept for payroll and tax obligations; the owner
/// handles those requests with legal advice.
pub async fn delete_candidate_data(pool: &PgPool, cfg: &PolicyConfig, actor: &Actor, candidate_id: Uuid, reason: &str) -> Result<(), PrivacyError> {
    if !actor.is_mfa_admin() {
        return Err(PrivacyError::Denied("deletion requests are handled by an MFA-verified admin".into()));
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT status FROM axom.candidates WHERE id = $1").bind(candidate_id).fetch_optional(pool).await?;
    let (status,) = row.ok_or(PrivacyError::NotFound(candidate_id))?;
    if HIRED.contains(&status.as_str()) {
        return Err(PrivacyError::Denied(format!("candidate is {status}: contract, payroll, and tax records must be retained; handle this request manually")));
    }
    let mut tx = pool.begin().await?;
    redact(&mut tx, candidate_id).await?;
    let ev = AuditEvent::new(actor, "privacy.deletion_request", "redacted").inputs(json!({ "candidate_id": candidate_id, "reason": reason })).policy_version(cfg.version());
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(())
}
