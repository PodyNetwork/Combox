//! Tool adapter traits and mock implementations.
//!
//! Milestone 2 defines the traits and in-memory mocks so the planner and
//! routing can run end to end. Real adapters (DocuSeal, Warmbly, Postiz, CRM,
//! SMTP, task intake) arrive in milestone 5, LiveKit in 6, Upwork in 8.
//!
//! Every outbound method takes an [`Authorized`] value, which can only be
//! built from an `allow` policy decision. Adapter output is untrusted data.

pub mod config;
pub mod integrations;
pub mod llm;
pub mod adapters;
pub mod intents;
pub mod mock;
pub mod near_rpc;
pub mod not_set_up;
pub mod price;
pub mod rhea;

use async_trait::async_trait;
use axom_core::untrusted::Untrusted;
use axom_core::decision::{Decision, Outcome};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Proof that the policy engine allowed an action. Adapters refuse to act
/// without one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Authorized {
    action: String,
    policy_version: String,
}

impl Authorized {
    /// Returns `None` unless the decision is `allow`.
    pub fn from_decision(action: impl Into<String>, decision: &Decision) -> Option<Self> {
        (decision.outcome == Outcome::Allow)
            .then(|| Self { action: action.into(), policy_version: decision.policy_version.clone() })
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }
}

/// Typed adapter errors the planner can act on.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolError {
    #[error("retryable error: {0}")]
    Retryable(String),
    #[error("permanent error: {0}")]
    Permanent(String),
    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("not authorized: {0}")]
    NotAuthorized(String),
}

pub type ToolResult<T> = Result<T, ToolError>;

/// The standard tool names roles and tasks use (config/planner.toml). Each
/// is a slot a company fills by connecting an MCP server (or webhook) with
/// that name; until then, tasks that need it wait.
pub const STANDARD_TOOLS: &[&str] = &["code_repo", "web_research", "postiz", "warmbly", "crm", "docuseal", "smtp", "upwork"];

/// What a standard tool slot is for (shown to Claude and on the dashboard).
pub fn standard_tool_description(tool: &str) -> &'static str {
    match tool {
        "code_repo" => "The company's code repository (for example GitHub's MCP server).",
        "web_research" => "Web search and reading pages.",
        "postiz" => "Social media posting (for example Postiz).",
        "warmbly" => "Email outreach campaigns (for example Warmbly).",
        "crm" => "Contacts and notes (for example opendesk).",
        "docuseal" => "Documents for signature (for example DocuSeal).",
        "smtp" => "Sending email.",
        "upwork" => "Posting jobs on Upwork.",
        _ => "A company tool.",
    }
}

/// One callable tool behind a name a worker may use.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    /// Passed to `ToolExecutor::execute`.
    pub id: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
}

impl ToolSpec {
    /// A tool that takes a free-text request.
    pub fn simple(id: &str, description: &str) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            input_schema: serde_json::json!({ "type": "object", "properties": { "request": { "type": "string", "description": "What you need from this tool" } }, "required": ["request"] }),
        }
    }
}

/// Generic tool execution used by AI worker agents. The output is untrusted.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, auth: &Authorized, tool: &str, input: Value) -> ToolResult<Untrusted>;

    /// The tools behind a name on a worker's list: one tool, or every tool of
    /// the MCP server connected under that name. `None` when nothing is
    /// connected. By default the name itself is the tool (test doubles).
    async fn resolve(&self, name: &str) -> ToolResult<Option<Vec<ToolSpec>>> {
        Ok(Some(vec![ToolSpec::simple(name, standard_tool_description(name))]))
    }
}

/// For names nothing is connected to: never runs anything.
pub struct Unconnected;

#[async_trait]
impl ToolExecutor for Unconnected {
    async fn execute(&self, _: &Authorized, tool: &str, _: Value) -> ToolResult<Untrusted> {
        Err(ToolError::Permanent(format!("{tool} is not connected: add an MCP server named {tool} on the Tools page")))
    }

    async fn resolve(&self, _: &str) -> ToolResult<Option<Vec<ToolSpec>>> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// DocuSeal (legal)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    Pending,
    Sent,
    Opened,
    Completed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxFormKind {
    W9,
    W8Ben,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signer {
    pub name: String,
    pub email: String,
}

#[async_trait]
pub trait ContractService: Send + Sync {
    /// Create a submission from an owner-approved template.
    async fn create_from_template(&self, auth: &Authorized, template: &str, signers: &[Signer], fields: Value) -> ToolResult<String>;
    async fn send(&self, auth: &Authorized, submission_id: &str) -> ToolResult<()>;
    async fn status(&self, submission_id: &str) -> ToolResult<SubmissionStatus>;
    async fn fetch_tax_form(&self, signer_email: &str) -> ToolResult<Option<TaxFormKind>>;
}

// ---------------------------------------------------------------------------
// Warmbly (sales)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub company: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplyStats {
    pub sent: u64,
    pub replies: u64,
    pub positive: u64,
}

#[async_trait]
pub trait SalesOutreach: Send + Sync {
    async fn create_campaign(&self, auth: &Authorized, name: &str, body: &str) -> ToolResult<String>;
    async fn update_campaign(&self, auth: &Authorized, campaign_id: &str, body: &str) -> ToolResult<()>;
    async fn add_contacts(&self, auth: &Authorized, campaign_id: &str, contacts: &[Contact]) -> ToolResult<u64>;
    async fn reply_stats(&self, campaign_id: &str) -> ToolResult<ReplyStats>;
}

// ---------------------------------------------------------------------------
// Postiz (social)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait SocialScheduler: Send + Sync {
    async fn schedule_post(&self, auth: &Authorized, channels: &[String], content: &str, at: DateTime<Utc>) -> ToolResult<String>;
}

// ---------------------------------------------------------------------------
// CRM (opendesk, adapters::opendesk)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait Crm: Send + Sync {
    async fn upsert_contact(&self, auth: &Authorized, contact: &Contact) -> ToolResult<String>;
    async fn upsert_deal(&self, auth: &Authorized, name: &str, contact_id: &str, value_usd: f64) -> ToolResult<String>;
    async fn add_note(&self, auth: &Authorized, record_id: &str, note: &str) -> ToolResult<String>;
}

// ---------------------------------------------------------------------------
// LiveKit meetings (implementation in milestone 6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomInfo {
    pub room: String,
    pub url: String,
}

#[async_trait]
pub trait MeetingRooms: Send + Sync {
    async fn create_room(&self, auth: &Authorized, name: &str) -> ToolResult<RoomInfo>;
    async fn participant_token(&self, auth: &Authorized, room: &str, identity: &str) -> ToolResult<String>;
    async fn end_room(&self, auth: &Authorized, room: &str) -> ToolResult<()>;
    /// Send the meeting agent into a room with job metadata.
    async fn dispatch_agent(&self, auth: &Authorized, room: &str, agent_name: &str, metadata: &str) -> ToolResult<String>;
    /// Whether an agent named `agent_name` is connected to the room right now
    /// (`false` when the room does not exist).
    async fn agent_in_room(&self, auth: &Authorized, room: &str, agent_name: &str) -> ToolResult<bool>;
}

// ---------------------------------------------------------------------------
// SMTP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Email {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Optional calendar invite (ICS text).
    #[serde(default)]
    pub ics: Option<String>,
}

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, auth: &Authorized, email: &Email) -> ToolResult<String>;
}

// ---------------------------------------------------------------------------
// Task intake (internal queue in axom-api)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntakeTask {
    pub task_id: uuid::Uuid,
    pub brief: String,
    pub required_skills: Vec<String>,
    pub assignee: Option<uuid::Uuid>,
}

#[async_trait]
pub trait TaskIntake: Send + Sync {
    /// Submit a task with its requirements; returns its queue position.
    async fn submit(&self, auth: &Authorized, task: &IntakeTask) -> ToolResult<u64>;
}

// ---------------------------------------------------------------------------
// Upwork via the hosted freelancing MCP (implementation in milestone 8)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub title: String,
    pub skills: Vec<String>,
    pub rate_usd: f64,
    pub kpis: Vec<String>,
    /// The job post's text (plain text; may be empty).
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applicant {
    pub id: String,
    pub name: String,
    /// Contact email, when the marketplace shares one.
    #[serde(default)]
    pub email: Option<String>,
    /// Skill tags from the applicant's profile (applicant-supplied: matched
    /// deterministically, never interpreted).
    #[serde(default)]
    pub skills: Vec<String>,
    /// The applicant's cover letter or message: untrusted.
    pub message: Untrusted,
}

#[async_trait]
pub trait FreelanceMarketplace: Send + Sync {
    /// Where postings go: `upwork` or `axom_board`. Freelancers from Upwork
    /// are paid through Upwork, never in NEAR (see `gates`).
    fn name(&self) -> &'static str {
        "upwork"
    }
    async fn post_job(&self, auth: &Authorized, spec: &JobSpec) -> ToolResult<String>;
    async fn list_applicants(&self, job_id: &str) -> ToolResult<Vec<Applicant>>;
    async fn message_candidate(&self, auth: &Authorized, applicant_id: &str, text: &str) -> ToolResult<()>;
}
