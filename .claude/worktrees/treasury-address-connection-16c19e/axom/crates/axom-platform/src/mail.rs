//! Platform emails: invitations to join a company, and subscription
//! reminders (a few days before a trial or paid period ends, and when it
//! has ended). Each goes through the policy engine like any other message
//! and through the configured mailer (dry-run unless AXOM_LIVE=true).

use std::sync::Arc;

use axom_core::domain::Actor;
use axom_hr::policy::{self, Action, Context, PolicyConfig};
use axom_tools::{Authorized, Email, Mailer};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::tenants::{audit, Platform, Subscription, Tenant};

/// How long before the end of a trial or paid period the reminder goes out.
pub const REMIND_DAYS_BEFORE: i64 = 3;

pub struct PlatformMail {
    pub mailer: Arc<dyn Mailer>,
    pub policy: Arc<PolicyConfig>,
    /// Base URL of the dashboard, e.g. `https://app.axom.example`.
    pub dashboard_url: String,
}

fn day(t: DateTime<Utc>) -> String {
    t.format("%B %-d, %Y").to_string()
}

impl PlatformMail {
    fn dashboard(&self) -> String {
        self.dashboard_url.trim_end_matches('/').to_string()
    }

    async fn send(&self, action: Action, auth_name: &str, email: Email) -> anyhow::Result<()> {
        let d = policy::evaluate(&self.policy, &action, &Context::new(Actor::System, Utc::now()));
        let auth = Authorized::from_decision(auth_name, &d).ok_or_else(|| anyhow::anyhow!("email refused by policy: {}", d.reason))?;
        self.mailer.send(&auth, &email).await?;
        Ok(())
    }

    /// Tell an invited person how to join. They join by signing up (or in)
    /// with this address; the email carries no secret link.
    pub async fn invite(&self, to: &str, company: &str, inviter: &str) -> anyhow::Result<()> {
        let to = to.to_string();
        let body = format!(
            "{inviter} invited you to help manage {company} on Combox.\n\n\
             To join, open {dashboard} and create an account (or sign in) with this email address, {to}. \
             You'll set up two-factor authentication with an authenticator app, then {company} appears in your list of companies.\n\n\
             If you weren't expecting this, you can ignore this email.",
            dashboard = self.dashboard(),
        );
        let email = Email { to: vec![to.clone()], subject: format!("You're invited to {company} on Combox"), body, ics: None };
        self.send(Action::SendExternalMessage { recipients: vec![to.clone()], allowed_recipients: vec![to] }, "send_external_message", email).await
    }

    /// The reminder due for a company now, if any: (kind, period end, subject, body).
    pub fn reminder_for(&self, t: &Tenant, now: DateTime<Utc>) -> Option<(&'static str, DateTime<Utc>, String, String)> {
        let billing = format!("{}/c/{}/billing", self.dashboard(), t.slug);
        let soon = |end: DateTime<Utc>| end > now && end - now <= Duration::days(REMIND_DAYS_BEFORE);
        match t.subscription(now) {
            Subscription::Trial { ends_at } if soon(ends_at) => Some((
                "trial_ending",
                ends_at,
                format!("Your Combox trial for {} ends {}", t.name, day(ends_at)),
                format!(
                    "Your free trial for {} ends on {}. To keep your AI workers, hiring, and payroll running, pay for a subscription in NEAR here:\n{billing}\n\nAfter the trial, automation pauses until you pay; your data is kept.",
                    t.name,
                    day(ends_at)
                ),
            )),
            Subscription::Active { paid_until } if soon(paid_until) => Some((
                "subscription_ending",
                paid_until,
                format!("Your Combox subscription for {} ends {}", t.name, day(paid_until)),
                format!("Your subscription for {} is paid until {}. Renew in NEAR here:\n{billing}", t.name, day(paid_until)),
            )),
            Subscription::Expired { since } => Some((
                "expired",
                since,
                format!("{} is paused on Combox", t.name),
                format!(
                    "The subscription for {} ended on {}, so its AI workers, hiring, and payroll are paused. Your data is kept. Renew in NEAR to resume:\n{billing}",
                    t.name,
                    day(since)
                ),
            )),
            _ => None,
        }
    }

    /// Send each due reminder once. Returns how many were sent.
    pub async fn send_reminders(&self, platform: &Platform, now: DateTime<Utc>) -> anyhow::Result<usize> {
        let mut sent = 0;
        for t in platform.active_tenants().await? {
            let Some((kind, end, subject, body)) = self.reminder_for(&t, now) else { continue };
            let (done,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM platform.reminders WHERE tenant_id = $1 AND kind = $2 AND period_end = $3)")
                .bind(t.id)
                .bind(kind)
                .bind(end)
                .fetch_one(&platform.db)
                .await?;
            if done {
                continue;
            }
            let email = Email { to: vec![t.owner_email.clone()], subject, body, ics: None };
            match self.send(Action::NotifyOwner { recipient: t.owner_email.clone(), owner_email: t.owner_email.clone() }, "notify_owner", email).await {
                Ok(()) => {
                    sqlx::query("INSERT INTO platform.reminders (tenant_id, kind, period_end) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                        .bind(t.id)
                        .bind(kind)
                        .bind(end)
                        .execute(&platform.db)
                        .await?;
                    audit(&platform.db, Some(t.id), "system", "billing.reminder", json!({ "kind": kind, "period_end": end })).await?;
                    sent += 1;
                }
                Err(e) => tracing::warn!(company = %t.slug, error = %e, "subscription reminder not sent; will retry"),
            }
        }
        Ok(sent)
    }
}
