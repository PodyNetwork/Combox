//! SMTP through the owner's server: summaries, owner alerts, and calendar
//! invites (ICS attachments). Credentials come from the environment and are
//! never logged.

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::sync::{Arc, Mutex};

use super::http::{live_from_env, need, Redactor};
use crate::{Authorized, Email, Mailer, ToolError, ToolResult};

pub struct SmtpMailer {
    from: String,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    redactor: Redactor,
    /// Messages built (dry-run) or sent (live), for inspection.
    pub outbox: Arc<Mutex<Vec<Email>>>,
}

impl std::fmt::Debug for SmtpMailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpMailer").field("from", &self.from).field("live", &self.transport.is_some()).finish_non_exhaustive()
    }
}

const ACTIONS: &[&str] = &["notify_owner", "send_external_message", "send_summary"];

impl SmtpMailer {
    /// Dry-run mailer: builds and validates messages, sends nothing.
    pub fn dry_run(from: &str) -> Self {
        Self { from: from.into(), transport: None, redactor: Redactor::default(), outbox: Arc::default() }
    }

    pub fn live(host: &str, port: u16, username: &str, password: &str, from: &str) -> anyhow::Result<Self> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?
            .port(port)
            .credentials(Credentials::new(username.into(), password.into()))
            .build();
        Ok(Self { from: from.into(), transport: Some(transport), redactor: Redactor::new([password.to_string(), username.to_string()]), outbox: Arc::default() })
    }

    /// `SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`.
    /// Dry-run unless `AXOM_LIVE=true`.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let from = need(c, "SMTP_FROM")?;
        if !live_from_env() {
            return Ok(Self::dry_run(&from));
        }
        let port = c.get("SMTP_PORT").and_then(|p| p.parse().ok()).unwrap_or(587);
        Self::live(&need(c, "SMTP_HOST")?, port, &need(c, "SMTP_USERNAME")?, &need(c, "SMTP_PASSWORD")?, &from)
    }

    pub fn build(&self, email: &Email) -> ToolResult<Message> {
        let mut b = Message::builder()
            .from(self.from.parse().map_err(|e| ToolError::Permanent(format!("bad from address: {e}")))?)
            .subject(email.subject.clone());
        if email.to.is_empty() {
            return Err(ToolError::Permanent("email has no recipients".into()));
        }
        for to in &email.to {
            b = b.to(to.parse().map_err(|e| ToolError::Permanent(format!("bad recipient {to}: {e}")))?);
        }
        let text = SinglePart::builder().header(ContentType::TEXT_PLAIN).body(email.body.clone());
        let msg = match &email.ics {
            Some(ics) => {
                let cal = Attachment::new("invite.ics".into()).body(ics.clone(), "text/calendar; method=REQUEST".parse().expect("content type"));
                b.multipart(MultiPart::mixed().singlepart(text).singlepart(cal))
            }
            None => b.singlepart(text),
        };
        msg.map_err(|e| ToolError::Permanent(format!("building email: {e}")))
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, auth: &Authorized, email: &Email) -> ToolResult<String> {
        if !ACTIONS.contains(&auth.action()) {
            return Err(ToolError::NotAuthorized(format!("smtp does not accept a decision for action {}", auth.action())));
        }
        let msg = self.build(email)?;
        self.outbox.lock().unwrap().push(email.clone());
        let Some(t) = &self.transport else { return Ok("dry-run".into()) };
        match t.send(msg).await {
            Ok(r) => Ok(r.message().collect::<Vec<_>>().join(" ")),
            Err(e) => {
                let text = self.redactor.clean(&e.to_string());
                Err(if e.is_transient() { ToolError::Retryable(format!("smtp: {text}")) } else { ToolError::Permanent(format!("smtp: {text}")) })
            }
        }
    }
}

/// Calendar alarms on every invite: one hour and ten minutes before the start.
const ALARMS: &str = "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Interview in 1 hour\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Interview in 10 minutes\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\n";

/// A minimal RFC 5545 invite, with reminder alarms.
pub fn ics_invite(uid: &str, title: &str, start: chrono::DateTime<chrono::Utc>, minutes: i64, organizer: &str, url: &str) -> String {
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y%m%dT%H%M%SZ").to_string();
    let end = start + chrono::Duration::minutes(minutes);
    let clean = |s: &str| s.replace(['\r', '\n'], " ").replace(',', "\\,").replace(';', "\\;");
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Combox//Meetings//EN\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{}\r\nORGANIZER:mailto:{}\r\nURL:{}\r\n{ALARMS}END:VEVENT\r\nEND:VCALENDAR\r\n",
        fmt(chrono::Utc::now()), fmt(start), fmt(end), clean(title), organizer, url
    )
}
