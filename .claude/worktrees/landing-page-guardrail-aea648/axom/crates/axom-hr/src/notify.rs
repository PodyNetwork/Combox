//! Owner notifications: a queue in `axom.owner_notifications`. Each row is the
//! dashboard feed entry; [`deliver_pending`] emails undelivered rows through
//! the SMTP adapter. Notifications are written in the same transaction as the
//! event they report, so they exist the moment the event commits; the
//! delivery loop runs every few seconds, well inside the one-minute limit.

use axom_core::domain::Actor;
use axom_tools::{Authorized, Email, Mailer};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::policy::{self, Action, Context, PolicyConfig};

/// Where notifications go and how audit links are built.
#[derive(Debug, Clone)]
pub struct NotifyConfig {
    pub owner_email: String,
    /// Base URL of the admin dashboard, e.g. `https://admin.example.com`.
    pub dashboard_url: String,
}

impl NotifyConfig {
    /// From `AXOM_OWNER_EMAIL` and `AXOM_DASHBOARD_URL`.
    pub fn from_env() -> Self {
        Self {
            owner_email: std::env::var("AXOM_OWNER_EMAIL").unwrap_or_default(),
            dashboard_url: std::env::var("AXOM_DASHBOARD_URL").unwrap_or_else(|_| "http://localhost:3000".into()),
        }
    }

    pub fn audit_link(&self, audit_id: Uuid) -> String {
        format!("{}/audit/{audit_id}", self.dashboard_url.trim_end_matches('/'))
    }
}

pub async fn enqueue<'e, E: PgExecutor<'e>>(
    ex: E,
    kind: &str,
    subject: &str,
    body: Value,
    audit_id: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.owner_notifications (id, kind, subject, body, audit_id) VALUES ($1,$2,$3,$4,$5)")
        .bind(id)
        .bind(kind)
        .bind(subject)
        .bind(body)
        .bind(audit_id)
        .execute(ex)
        .await?;
    Ok(id)
}

pub async fn count_kind<'e, E: PgExecutor<'e>>(ex: E, kind: &str) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.owner_notifications WHERE kind = $1")
        .bind(kind)
        .fetch_one(ex)
        .await?;
    Ok(n)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub subject: String,
    pub body: Value,
    pub audit_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub email_sent_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub read_at: Option<DateTime<Utc>>,
}

const COLS: &str = "id, kind, subject, body, audit_id, created_at, email_sent_at, attempts, read_at";

/// Dashboard feed, newest first.
pub async fn feed(pool: &PgPool, limit: i64) -> Result<Vec<Notification>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM axom.owner_notifications ORDER BY created_at DESC, id LIMIT $1"))
        .bind(limit)
        .fetch_all(pool)
        .await
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Notification>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM axom.owner_notifications WHERE id = $1")).bind(id).fetch_optional(pool).await
}

/// Render a notification as a plain-text email body.
pub fn render_email(n: &Notification, cfg: &NotifyConfig) -> String {
    let mut out = format!("{}\n\n", n.subject);
    if let Some(reason) = n.body.get("reason").and_then(Value::as_str) {
        out.push_str(&format!("Reason: {reason}\n\n"));
    }
    if let Some(metrics) = n.body.get("metrics") {
        out.push_str("Metrics:\n");
        out.push_str(&serde_json::to_string_pretty(metrics).unwrap_or_default());
        out.push_str("\n\n");
    }
    out.push_str("Details:\n");
    out.push_str(&serde_json::to_string_pretty(&n.body).unwrap_or_default());
    out.push('\n');
    if let Some(a) = n.audit_id {
        out.push_str(&format!("\nAudit record: {}\n", cfg.audit_link(a)));
    }
    out
}

/// Maximum delivery attempts before a notification stays in the feed only.
pub const MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeliveryReport {
    pub sent: Vec<Uuid>,
    pub failed: Vec<Uuid>,
}

/// Email every undelivered notification to the owner.
pub async fn deliver_pending(pool: &PgPool, policy_cfg: &PolicyConfig, cfg: &NotifyConfig, mailer: &dyn Mailer) -> anyhow::Result<DeliveryReport> {
    let pending: Vec<Notification> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM axom.owner_notifications WHERE email_sent_at IS NULL AND attempts < $1 ORDER BY created_at, id"
    ))
    .bind(MAX_ATTEMPTS)
    .fetch_all(pool)
    .await?;
    let mut report = DeliveryReport::default();
    if pending.is_empty() {
        return Ok(report);
    }
    let decision = policy::evaluate(
        policy_cfg,
        &Action::NotifyOwner { recipient: cfg.owner_email.clone(), owner_email: cfg.owner_email.clone() },
        &Context::new(Actor::System, Utc::now()),
    );
    let Some(auth) = Authorized::from_decision("notify_owner", &decision) else {
        anyhow::bail!("owner notifications cannot be emailed: {}", decision.reason);
    };
    for n in pending {
        let email = Email { to: vec![cfg.owner_email.clone()], subject: format!("[Combox] {}", n.subject), body: render_email(&n, cfg), ics: None };
        match mailer.send(&auth, &email).await {
            Ok(_) => {
                sqlx::query("UPDATE axom.owner_notifications SET email_sent_at = now(), delivered_at = now(), attempts = attempts + 1 WHERE id = $1")
                    .bind(n.id)
                    .execute(pool)
                    .await?;
                report.sent.push(n.id);
            }
            Err(e) => {
                sqlx::query("UPDATE axom.owner_notifications SET attempts = attempts + 1, last_error = $2 WHERE id = $1")
                    .bind(n.id)
                    .bind(e.to_string())
                    .execute(pool)
                    .await?;
                report.failed.push(n.id);
            }
        }
    }
    Ok(report)
}
