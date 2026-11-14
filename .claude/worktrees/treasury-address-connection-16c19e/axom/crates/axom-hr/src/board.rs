//! Combox Jobs: Combox's own jobs board, used in place of Upwork.
//!
//! Upwork forbids paying freelancers it introduced outside its platform
//! (without a conversion fee), so they can't be paid in NEAR. Freelancers who
//! apply here have no such tie: once hired, they pass the same gates (a
//! verified NEAR address and a tax form) and are paid by the signer.
//!
//! The board implements [`FreelanceMarketplace`], so the hiring pipeline
//! (policy check, shortlist, AI interview, contract, activation) is the same
//! as for Upwork. Jobs are published on the public board; applicants apply
//! through the public API; Combox contacts them by email.
//!
//! Everything an applicant submits is untrusted data: stored and matched
//! deterministically, never interpreted.

use std::sync::Arc;

use async_trait::async_trait;
use axom_tools::{Applicant, Authorized, Email, FreelanceMarketplace, JobSpec, Mailer, ToolError, ToolResult};
use axom_core::untrusted::Untrusted;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub const MARKETPLACE: &str = "axom_board";

/// Most applications accepted per job; later ones are refused.
pub const MAX_APPLICATIONS_PER_JOB: i64 = 500;

pub struct AxomBoard {
    pool: PgPool,
    mailer: Arc<dyn Mailer>,
}

impl AxomBoard {
    pub fn new(pool: PgPool, mailer: Arc<dyn Mailer>) -> Self {
        Self { pool, mailer }
    }
}

fn db_err(e: sqlx::Error) -> ToolError {
    ToolError::Retryable(format!("jobs board database: {e}"))
}

fn require(auth: &Authorized, action: &str) -> ToolResult<()> {
    if auth.action() == action {
        Ok(())
    } else {
        Err(ToolError::NotAuthorized(format!("jobs board: needs a {action} authorization, got {}", auth.action())))
    }
}

#[async_trait]
impl FreelanceMarketplace for AxomBoard {
    fn name(&self) -> &'static str {
        MARKETPLACE
    }

    async fn post_job(&self, auth: &Authorized, spec: &JobSpec) -> ToolResult<String> {
        require(auth, "hire")?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO axom.board_jobs (id, title, skills, rate_usd, kpis, description) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(id)
            .bind(&spec.title)
            .bind(&spec.skills)
            .bind(Decimal::from_f64(spec.rate_usd).unwrap_or_default().round_dp(2))
            .bind(&spec.kpis)
            .bind(&spec.description)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(id.to_string())
    }

    async fn list_applicants(&self, job_id: &str) -> ToolResult<Vec<Applicant>> {
        let job: Uuid = job_id.parse().map_err(|_| ToolError::Permanent(format!("jobs board: bad job id {job_id}")))?;
        let rows: Vec<(Uuid, String, String, Vec<String>, String)> =
            sqlx::query_as("SELECT id, name, email, skills, message FROM axom.board_applications WHERE job_id = $1 ORDER BY created_at")
                .bind(job)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(id, name, email, skills, message)| Applicant { id: id.to_string(), name, email: Some(email), skills, message: Untrusted::new("axom_board_application", message) })
            .collect())
    }

    async fn message_candidate(&self, auth: &Authorized, applicant_id: &str, text: &str) -> ToolResult<()> {
        require(auth, "send_external_message")?;
        let id: Uuid = applicant_id.parse().map_err(|_| ToolError::Permanent(format!("jobs board: bad applicant id {applicant_id}")))?;
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT a.email, j.title FROM axom.board_applications a JOIN axom.board_jobs j ON j.id = a.job_id WHERE a.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        let (email, title) = row.ok_or_else(|| ToolError::Permanent(format!("jobs board: applicant {applicant_id} not found")))?;
        self.mailer.send(auth, &Email { to: vec![email], subject: format!("Your application: {title}"), body: text.to_string(), ics: None }).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public board (read by anyone, applications from anyone)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PublicJob {
    pub id: Uuid,
    pub title: String,
    pub skills: Vec<String>,
    pub rate_usd: Decimal,
    pub kpis: Vec<String>,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

/// Open jobs, newest first.
pub async fn open_jobs(pool: &PgPool) -> Result<Vec<PublicJob>, sqlx::Error> {
    sqlx::query_as("SELECT id, title, skills, rate_usd, kpis, description, created_at FROM axom.board_jobs WHERE status = 'open' ORDER BY created_at DESC LIMIT 200")
        .fetch_all(pool)
        .await
}

pub async fn open_job(pool: &PgPool, id: Uuid) -> Result<Option<PublicJob>, sqlx::Error> {
    sqlx::query_as("SELECT id, title, skills, rate_usd, kpis, description, created_at FROM axom.board_jobs WHERE id = $1 AND status = 'open'")
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[derive(Debug, Clone, Deserialize)]
pub struct Application {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub portfolio_url: Option<String>,
    /// Required: the applicant agrees to the privacy notice (retention,
    /// AI interview with consent, email contact).
    #[serde(default)]
    pub accept_privacy: bool,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("{0}")]
    Invalid(String),
    #[error("this job is not open")]
    NotOpen,
    #[error("you have already applied for this job")]
    Duplicate,
    #[error("this job is not taking more applications")]
    Full,
    #[error("database error")]
    Db,
}

fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control() || c == ',' || c == ';' || c == '<' || c == '>')
        && !domain.contains('@')
}

/// Normalise a skill tag: lowercase, spaces and dashes to underscores.
fn skill_tag(s: &str) -> String {
    s.trim().to_lowercase().chars().map(|c| if c == ' ' || c == '-' { '_' } else { c }).filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect()
}

/// Check and clean an application.
pub fn validate(a: &Application) -> Result<Application, ApplyError> {
    let name = a.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(ApplyError::Invalid("name: 1 to 120 characters".into()));
    }
    let email = a.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(ApplyError::Invalid("a valid email address is required".into()));
    }
    if a.skills.len() > 20 {
        return Err(ApplyError::Invalid("at most 20 skills".into()));
    }
    let mut skills: Vec<String> = a.skills.iter().map(|s| skill_tag(s)).filter(|s| !s.is_empty() && s.len() <= 40).collect();
    skills.sort();
    skills.dedup();
    if a.message.chars().count() > 5000 {
        return Err(ApplyError::Invalid("message: at most 5000 characters".into()));
    }
    let portfolio_url = match a.portfolio_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        None => None,
        Some(u) if (u.starts_with("https://") || u.starts_with("http://")) && u.len() <= 500 && !u.contains(char::is_whitespace) => Some(u.to_string()),
        Some(_) => return Err(ApplyError::Invalid("portfolio link must be an http(s) URL of at most 500 characters".into())),
    };
    if !a.accept_privacy {
        return Err(ApplyError::Invalid("please accept the privacy notice to apply".into()));
    }
    Ok(Application { name: name.to_string(), email, skills, message: a.message.trim().to_string(), portfolio_url, accept_privacy: true })
}

/// Largest CV accepted.
pub const MAX_CV_BYTES: usize = 5 * 1024 * 1024;

/// A CV uploaded with an application (a PDF).
pub struct Cv {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Check a CV: a PDF of at most 5 MB. Returns a safe file name.
pub fn validate_cv(cv: &Cv) -> Result<String, ApplyError> {
    if cv.bytes.is_empty() {
        return Err(ApplyError::Invalid("please attach your CV as a PDF".into()));
    }
    if cv.bytes.len() > MAX_CV_BYTES {
        return Err(ApplyError::Invalid("your CV must be at most 5 MB".into()));
    }
    // Every PDF starts with "%PDF-" (some tools put a few bytes before it).
    if !cv.bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err(ApplyError::Invalid("your CV must be a PDF file".into()));
    }
    let base = cv.filename.rsplit(['/', '\\']).next().unwrap_or("");
    let stem: String = base
        .trim_end_matches(".pdf")
        .trim_end_matches(".PDF")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.'))
        .take(80)
        .collect();
    let stem = stem.trim();
    Ok(format!("{}.pdf", if stem.is_empty() { "cv" } else { stem }))
}

/// Store an application for an open job. Returns the application id.
pub async fn apply(pool: &PgPool, job_id: Uuid, application: &Application) -> Result<Uuid, ApplyError> {
    apply_with_cv(pool, job_id, application, None).await
}

/// Store an application, with the applicant's CV when given.
pub async fn apply_with_cv(pool: &PgPool, job_id: Uuid, application: &Application, cv: Option<&Cv>) -> Result<Uuid, ApplyError> {
    let a = validate(application)?;
    let cv_name = cv.map(validate_cv).transpose()?;
    let mut tx = pool.begin().await.map_err(|_| ApplyError::Db)?;
    // Lock the job row so the per-job limit holds under concurrent applies.
    let open: Option<(String,)> = sqlx::query_as("SELECT status FROM axom.board_jobs WHERE id = $1 FOR UPDATE")
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApplyError::Db)?;
    if open.map(|r| r.0).as_deref() != Some("open") {
        return Err(ApplyError::NotOpen);
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.board_applications WHERE job_id = $1")
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApplyError::Db)?;
    if n >= MAX_APPLICATIONS_PER_JOB {
        return Err(ApplyError::Full);
    }
    let id = Uuid::new_v4();
    let r = sqlx::query(
        "INSERT INTO axom.board_applications (id, job_id, name, email, skills, message, portfolio_url) VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (job_id, email) DO NOTHING",
    )
    .bind(id)
    .bind(job_id)
    .bind(&a.name)
    .bind(&a.email)
    .bind(&a.skills)
    .bind(&a.message)
    .bind(&a.portfolio_url)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApplyError::Db)?;
    if r.rows_affected() == 0 {
        return Err(ApplyError::Duplicate);
    }
    if let (Some(cv), Some(name)) = (cv, cv_name) {
        use sha2::Digest;
        sqlx::query("INSERT INTO axom.board_application_cvs (application_id, filename, content, size_bytes, sha256) VALUES ($1,$2,$3,$4,$5)")
            .bind(id)
            .bind(name)
            .bind(&cv.bytes)
            .bind(cv.bytes.len() as i32)
            .bind(hex::encode(sha2::Sha256::digest(&cv.bytes)))
            .execute(&mut *tx)
            .await
            .map_err(|_| ApplyError::Db)?;
    }
    tx.commit().await.map_err(|_| ApplyError::Db)?;
    Ok(id)
}

/// Close board jobs whose posting is no longer open (filled or closed).
pub async fn close_finished_jobs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE axom.board_jobs b SET status = 'closed', closed_at = now()
         FROM axom.job_postings j
         WHERE j.marketplace = 'axom_board' AND j.external_job_id = b.id::text AND j.status <> 'open' AND b.status = 'open'",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Application {
        Application {
            name: " Ada ".into(),
            email: "Ada@Example.com ".into(),
            skills: vec!["Visual Design".into(), "branding".into(), "branding".into()],
            message: "hi".into(),
            portfolio_url: None,
            accept_privacy: true,
        }
    }

    #[test]
    fn validation_cleans_and_rejects() {
        let a = validate(&app()).unwrap();
        assert_eq!(a.name, "Ada");
        assert_eq!(a.email, "ada@example.com");
        assert_eq!(a.skills, vec!["branding".to_string(), "visual_design".to_string()]);
        for bad in ["", "no-at", "a@b", "a@.com", "a b@c.com", "a@b.com, x@y.com"] {
            assert!(validate(&Application { email: bad.into(), ..app() }).is_err(), "{bad}");
        }
        assert!(validate(&Application { accept_privacy: false, ..app() }).is_err());
        assert!(validate(&Application { portfolio_url: Some("javascript:alert(1)".into()), ..app() }).is_err());
        assert!(validate(&Application { message: "x".repeat(5001), ..app() }).is_err());
    }
}
