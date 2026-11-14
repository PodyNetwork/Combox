//! The platform operator's view: every company, its subscription, members,
//! and payments; and suspending or resuming a company. Operators are the
//! emails in `AXOM_PLATFORM_OPERATORS` (comma-separated), signed in with
//! two-factor. A suspended company's API, jobs board, background jobs, and
//! payouts stop; its members still see their billing and members pages.

use axom_hr::approvals::AdminSession;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;

use crate::tenants::{audit, Platform, Subscription, TenantError};

#[derive(Debug, Clone, Default)]
pub struct Operators(Vec<String>);

impl Operators {
    pub fn new<I: IntoIterator<Item = S>, S: AsRef<str>>(emails: I) -> Self {
        Self(emails.into_iter().map(|e| e.as_ref().trim().to_lowercase()).filter(|e| e.contains('@')).collect())
    }

    pub fn from_env() -> Self {
        Self::new(std::env::var("AXOM_PLATFORM_OPERATORS").unwrap_or_default().split(','))
    }

    /// Whether this session belongs to an operator (two-factor required).
    pub fn allows(&self, s: &AdminSession) -> bool {
        s.mfa_verified && !s.email.is_empty() && self.0.contains(&s.email.trim().to_lowercase())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CompanyRow {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
    pub subscription: Subscription,
    pub members: i64,
    pub paid_usd: Decimal,
    pub paid_invoices: i64,
    pub has_treasury: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub companies: usize,
    pub trial: usize,
    pub active: usize,
    pub expired: usize,
    pub suspended: usize,
    pub paid_usd: Decimal,
}

impl Platform {
    /// All companies (set up ones), newest first, with totals.
    pub async fn operator_overview(&self, now: DateTime<Utc>) -> Result<(Vec<CompanyRow>, Totals), sqlx::Error> {
        type Row = (String, String, String, String, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, bool, i64, Decimal, i64);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT t.slug, t.name, t.status, t.owner_email, t.created_at, t.trial_ends_at, t.paid_until, t.treasury_key IS NOT NULL,
                    (SELECT count(*) FROM platform.members m WHERE m.tenant_id = t.id),
                    COALESCE((SELECT sum(amount_usd) FROM platform.invoices i WHERE i.tenant_id = t.id AND i.status = 'paid'), 0),
                    (SELECT count(*) FROM platform.invoices i WHERE i.tenant_id = t.id AND i.status = 'paid')
             FROM platform.tenants t WHERE t.status <> 'provisioning' ORDER BY t.created_at DESC",
        )
        .fetch_all(&self.db)
        .await?;
        let mut totals = Totals { companies: rows.len(), trial: 0, active: 0, expired: 0, suspended: 0, paid_usd: Decimal::ZERO };
        let list = rows
            .into_iter()
            .map(|(slug, name, status, owner_email, created_at, trial_ends_at, paid_until, has_treasury, members, paid_usd, paid_invoices)| {
                let subscription = match paid_until {
                    Some(p) if p > now => Subscription::Active { paid_until: p },
                    _ if trial_ends_at > now => Subscription::Trial { ends_at: trial_ends_at },
                    Some(p) => Subscription::Expired { since: p.max(trial_ends_at) },
                    None => Subscription::Expired { since: trial_ends_at },
                };
                if status == "suspended" {
                    totals.suspended += 1;
                } else {
                    match subscription {
                        Subscription::Trial { .. } => totals.trial += 1,
                        Subscription::Active { .. } => totals.active += 1,
                        Subscription::Expired { .. } => totals.expired += 1,
                    }
                }
                totals.paid_usd += paid_usd;
                CompanyRow { slug, name, status, owner_email, created_at, subscription, members, paid_usd, paid_invoices, has_treasury }
            })
            .collect();
        Ok((list, totals))
    }

    /// Suspend or resume a company (operator only; the caller checks).
    pub async fn set_suspended(&self, slug: &str, suspend: bool, operator: &AdminSession, reason: &str) -> Result<(), TenantError> {
        let reason = reason.trim();
        if suspend && (reason.is_empty() || reason.chars().count() > 500) {
            return Err(TenantError::Invalid("a reason (up to 500 characters) is required to suspend a company".into()));
        }
        let (from, to) = if suspend { ("active", "suspended") } else { ("suspended", "active") };
        let r: Option<(uuid::Uuid,)> = sqlx::query_as("UPDATE platform.tenants SET status = $3 WHERE slug = $1 AND status = $2 RETURNING id")
            .bind(slug)
            .bind(from)
            .bind(to)
            .fetch_optional(&self.db)
            .await?;
        let Some((id,)) = r else {
            return Err(TenantError::Invalid(format!("the company isn't {from}")));
        };
        audit(&self.db, Some(id), &format!("operator:{}", operator.email), if suspend { "company.suspend" } else { "company.resume" }, json!({ "reason": reason })).await?;
        Ok(())
    }
}
