//! What stands in for an outside service that can't be used in live mode
//! (missing or unusable settings).
//!
//! Outside live mode the mock is fine: nothing leaves the process anyway. In
//! live mode a mock would report emails as sent, contracts as signed and jobs
//! as posted when nothing happened. `NotSetUp` refuses every call instead, so
//! the failure is visible where the work is tracked.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
    Applicant, Authorized, ContractService, Email, FreelanceMarketplace, JobSpec, Mailer, MeetingRooms, RoomInfo, Signer, SocialScheduler, SubmissionStatus,
    TaxFormKind, ToolError, ToolResult,
};

#[derive(Debug, Clone)]
pub struct NotSetUp {
    service: &'static str,
    why: String,
}

impl NotSetUp {
    /// `service` is what the owner calls it ("email", "DocuSeal"); `why` is
    /// the settings error. Settings errors name the missing setting, never
    /// its value.
    pub fn new(service: &'static str, why: impl std::fmt::Display) -> Self {
        Self { service, why: why.to_string() }
    }

    fn refuse<T>(&self) -> ToolResult<T> {
        Err(ToolError::Permanent(format!("{} is not set up, so nothing was sent: {}", self.service, self.why)))
    }
}

#[async_trait]
impl Mailer for NotSetUp {
    async fn send(&self, _: &Authorized, _: &Email) -> ToolResult<String> {
        self.refuse()
    }
}

#[async_trait]
impl ContractService for NotSetUp {
    async fn create_from_template(&self, _: &Authorized, _: &str, _: &[Signer], _: Value) -> ToolResult<String> {
        self.refuse()
    }
    async fn send(&self, _: &Authorized, _: &str) -> ToolResult<()> {
        self.refuse()
    }
    async fn status(&self, _: &str) -> ToolResult<SubmissionStatus> {
        self.refuse()
    }
    async fn fetch_tax_form(&self, _: &str) -> ToolResult<Option<TaxFormKind>> {
        self.refuse()
    }
}

#[async_trait]
impl MeetingRooms for NotSetUp {
    async fn create_room(&self, _: &Authorized, _: &str) -> ToolResult<RoomInfo> {
        self.refuse()
    }
    async fn participant_token(&self, _: &Authorized, _: &str, _: &str) -> ToolResult<String> {
        self.refuse()
    }
    async fn end_room(&self, _: &Authorized, _: &str) -> ToolResult<()> {
        self.refuse()
    }
    async fn dispatch_agent(&self, _: &Authorized, _: &str, _: &str, _: &str) -> ToolResult<String> {
        self.refuse()
    }
    async fn agent_in_room(&self, _: &Authorized, _: &str, _: &str) -> ToolResult<bool> {
        self.refuse()
    }
}

#[async_trait]
impl FreelanceMarketplace for NotSetUp {
    async fn post_job(&self, _: &Authorized, _: &JobSpec) -> ToolResult<String> {
        self.refuse()
    }
    async fn list_applicants(&self, _: &str) -> ToolResult<Vec<Applicant>> {
        self.refuse()
    }
    async fn message_candidate(&self, _: &Authorized, _: &str, _: &str) -> ToolResult<()> {
        self.refuse()
    }
}

#[async_trait]
impl SocialScheduler for NotSetUp {
    async fn schedule_post(&self, _: &Authorized, _: &[String], _: &str, _: DateTime<Utc>) -> ToolResult<String> {
        self.refuse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axom_core::decision::{Decision, Outcome};

    #[tokio::test]
    async fn every_call_is_refused_and_says_why() {
        let d = Decision { outcome: Outcome::Allow, reason: "ok".into(), policy_version: "t".into(), notify_owner: false };
        let auth = Authorized::from_decision("send_external_message", &d).unwrap();
        let n = NotSetUp::new("email", "SMTP_HOST is not set");
        let email = Email { to: vec!["a@example.com".into()], subject: "s".into(), body: "b".into(), ics: None };
        let err = Mailer::send(&n, &auth, &email).await.unwrap_err();
        assert!(matches!(&err, ToolError::Permanent(m) if m.contains("email") && m.contains("SMTP_HOST")), "{err:?}");
        assert!(ContractService::status(&n, "1").await.is_err());
        assert!(n.list_applicants("job").await.is_err());
        assert!(n.create_room(&auth, "room").await.is_err());
    }
}
