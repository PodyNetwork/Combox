//! Meetings and interviews on the self-hosted LiveKit server.
//!
//! - `create_meeting` creates the room and one join link per invitee.
//! - `dispatch_agent` assigns the meeting agent its instructions, the
//!   owner's own questions (optional), and a job-related rubric. Rubric
//!   criteria that name protected characteristics are refused. The agent
//!   writes its interview questions itself from the role, asking the owner's
//!   first when there are any.
//! - Whoever joins (the applicant or the owner from the dashboard), the agent
//!   is in the room: [`ensure_agent`] sends it on the first join and sends a
//!   new one whenever it is no longer there (the room closed while empty, or
//!   the agent left after everyone did).
//! - The agent reports back through `record_callback` (per-meeting token):
//!   consent first, then the transcript, summary, and rubric scores. A
//!   transcript without recorded consent is rejected.
//! - Interviews are timed: [`INTERVIEW_MINUTES`] (15). A join link works
//!   from the moment it is sent until [`JOIN_LATE_MINUTES`] (24 hours) after
//!   the scheduled time; the interview starts whenever the invitee joins.
//!   The first invitee to join starts the clock: every invitee link stops working once the meeting's
//!   time limit has passed since then, and the agent enforces the same limit.
//! - [`expire_meetings`] marks meetings nobody finished as `expired` and
//!   closes their rooms; [`send_due_reminders`] reminds invitees 24 hours,
//!   1 hour, and 10 minutes before the start, each with a fresh join link.
//! - `send_summary` goes only to the invitees and the owner, plus recipients
//!   the owner added in the dashboard, and is saved as a CRM note.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::untrusted::Untrusted;
use axom_tools::{Authorized, Contact, Crm, Email, FreelanceMarketplace, Mailer, MeetingRooms};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::notify::NotifyConfig;
use crate::policy::{self, Action, Context, PolicyConfig};

#[derive(Debug, thiserror::Error)]
pub enum MeetingError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("not allowed: {0}")]
    Denied(String),
    #[error("meeting {0} not found")]
    NotFound(Uuid),
    #[error("unauthorized callback")]
    BadToken,
    #[error("tool error: {0}")]
    Tool(#[from] axom_tools::ToolError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub const AGENT_NAME: &str = "axom-interviewer";
/// Hard limit on an interview, enforced by the meeting agent. Invitee links
/// also stop working this long after the first invitee joined.
pub const INTERVIEW_MINUTES: i64 = 15;
/// A join link works from the moment it is sent until this long after the
/// scheduled time (24 hours). Then the link has expired.
pub const JOIN_LATE_MINUTES: i64 = 24 * 60;
/// Extra time after the latest possible end before a meeting is expired.
pub const EXPIRE_GRACE_MINUTES: i64 = 10;
/// Reminder slots: (name, minutes before the start), earliest first.
pub const REMINDERS: &[(&str, i64)] = &[("24h", 24 * 60), ("1h", 60), ("10m", 10)];
/// Invitee address used for Upwork applicants who did not share an email;
/// they are reached through the marketplace instead.
const PLACEHOLDER_DOMAIN: &str = "@candidates.axom.invalid";

/// When a meeting's invitee join links expire (they work until then, from
/// the moment they are sent).
pub fn link_expiry(scheduled_at: DateTime<Utc>) -> DateTime<Utc> {
    scheduled_at + Duration::minutes(JOIN_LATE_MINUTES)
}

fn utc(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

async fn insert_invite<'e, E: sqlx::PgExecutor<'e>>(ex: E, meeting_id: Uuid, email: &str, expires_at: DateTime<Utc>) -> Result<String, sqlx::Error> {
    let token = rand_token("inv");
    sqlx::query("INSERT INTO axom.meeting_invites (token_sha256, meeting_id, email, expires_at) VALUES ($1,$2,$3,$4)")
        .bind(sha(&token))
        .bind(meeting_id)
        .bind(email)
        .bind(expires_at)
        .execute(ex)
        .await?;
    Ok(token)
}

/// What the agent says first, before anything else.
pub const DISCLOSURE: &str = "Hello, I'm an AI interviewer working on behalf of the hiring team. \
This conversation is with an AI, not a person. With your permission, it will be recorded and transcribed \
so the team can review it. Do you consent to this interview being recorded?";

/// Characteristics that may never be evaluation criteria.
const PROTECTED: &[&str] = &[
    "age", "gender", "sex", "race", "ethnic", "national", "nationality", "religion", "religious", "disab",
    "pregnan", "marital", "family", "sexual", "orientation", "color", "colour", "citizenship", "veteran", "genetic", "accent", "appearance",
];

fn rand_token(prefix: &str) -> String {
    let mut b = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut b);
    format!("{prefix}_{}", hex::encode(b))
}

/// Stable pseudonym for an email in the append-only audit log.
pub fn pseudonym(email: &str) -> String {
    format!("email:{}", &sha(&email.trim().to_lowercase())[..16])
}

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 160 && e.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.') && !d.starts_with('.') && !e.contains(char::is_whitespace))
}

fn allow(cfg: &PolicyConfig, ctx: &Context, op: &str) -> Result<Authorized, MeetingError> {
    let d = policy::evaluate(cfg, &Action::Meeting { op: op.into() }, ctx);
    Authorized::from_decision(op, &d).ok_or(MeetingError::Denied(d.reason))
}

async fn ctx(pool: &PgPool, actor: &Actor) -> Result<Context, sqlx::Error> {
    let mut c = Context::new(actor.clone(), Utc::now());
    c.kill_switch = crate::payouts::kill_switch_on(pool).await?;
    Ok(c)
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingCreated {
    pub meeting_id: Uuid,
    pub room: String,
    /// (invitee email, join token). The token goes into the join link only.
    pub invites: Vec<(String, String)>,
}

#[allow(clippy::too_many_arguments)]
pub async fn create_meeting(
    pool: &PgPool,
    cfg: &PolicyConfig,
    rooms: &dyn MeetingRooms,
    actor: &Actor,
    title: &str,
    scheduled_at: DateTime<Utc>,
    invitees: &[String],
    candidate_worker_id: Option<Uuid>,
) -> Result<MeetingCreated, MeetingError> {
    let title = title.trim();
    if title.is_empty() || title.len() > 200 {
        return Err(MeetingError::Invalid("title must be 1-200 characters".into()));
    }
    if invitees.is_empty() || invitees.len() > 20 || !invitees.iter().all(|e| valid_email(e)) {
        return Err(MeetingError::Invalid("1-20 valid invitee emails required".into()));
    }
    let expires_at = link_expiry(scheduled_at);
    if expires_at <= Utc::now() {
        return Err(MeetingError::Invalid("the scheduled time has already passed".into()));
    }
    let c = ctx(pool, actor).await?;
    let auth = allow(cfg, &c, "create_meeting")?;
    let id = Uuid::new_v4();
    let room = format!("axom-{}", id.simple());
    rooms.create_room(&auth, &room).await?;
    let invitees: Vec<String> = invitees.iter().map(|e| e.trim().to_lowercase()).collect();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO axom.meetings (id, title, room, scheduled_at, invitees, candidate_worker_id, duration_minutes) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(id)
        .bind(title)
        .bind(&room)
        .bind(scheduled_at)
        .bind(&invitees)
        .bind(candidate_worker_id)
        .bind(INTERVIEW_MINUTES as i32)
        .execute(&mut *tx)
        .await?;
    let mut invites = Vec::new();
    for email in &invitees {
        let token = insert_invite(&mut *tx, id, email, expires_at).await?;
        invites.push((email.clone(), token));
    }
    let ev = AuditEvent::new(actor, "meeting.create", "created")
        // Pseudonymous references only: the audit log is append-only, so it must
        // not hold personal data that a deletion request could not remove.
        .inputs(json!({
            "meeting_id": id, "room": room, "scheduled_at": scheduled_at,
            "invitees": invitees.iter().map(|e| pseudonym(e)).collect::<Vec<_>>(),
            "duration_minutes": INTERVIEW_MINUTES, "links_expire_at": expires_at,
        }))
        .policy_version(cfg.version());
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(MeetingCreated { meeting_id: id, room, invites })
}

/// Email each invitee their own join link with a calendar invite (ICS).
/// Placeholder addresses (`*.invalid`, used when a marketplace shares no
/// email) are skipped; those candidates get the link over the marketplace.
/// A meeting with no interviewer questions yet gets the defaults first
/// ([`ensure_interviewer`]), so no link goes out for an interview the agent
/// could never join.
#[allow(clippy::too_many_arguments)]
pub async fn send_invites(
    pool: &PgPool,
    cfg: &PolicyConfig,
    mailer: &dyn Mailer,
    actor: &Actor,
    meeting_id: Uuid,
    invites: &[(String, String)],
    meet_web_url: &str,
    from_address: &str,
) -> Result<usize, MeetingError> {
    ensure_interviewer(pool, cfg, actor, meeting_id).await?;
    let (title, at): (String, DateTime<Utc>) = sqlx::query_as("SELECT title, scheduled_at FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_one(pool).await?;
    let recipients: Vec<String> = invites.iter().map(|(e, _)| e.clone()).filter(|e| !e.ends_with(".invalid")).collect();
    if recipients.is_empty() {
        return Ok(0);
    }
    let c = ctx(pool, actor).await?;
    // Invites go only to the meeting's own invitees.
    let d = policy::evaluate(cfg, &Action::SendExternalMessage { recipients: recipients.clone(), allowed_recipients: recipients.clone() }, &c);
    let auth = Authorized::from_decision("send_external_message", &d).ok_or_else(|| MeetingError::Denied(d.reason.clone()))?;
    let mut sent = 0;
    for (email, token) in invites.iter().filter(|(e, _)| recipients.contains(e)) {
        let link = format!("{}/join?t={token}", meet_web_url.trim_end_matches('/'));
        let ics = axom_tools::adapters::smtp::ics_invite(&format!("{meeting_id}-{email}"), &title, at, 45, from_address, &link);
        let body = format!(
            "You're invited to \"{title}\" on {} UTC.\n\nJoin here: {link}\n\nThis meeting may include an AI interviewer. It will tell you it is an AI and ask for your consent before anything is recorded.",
            at.format("%Y-%m-%d %H:%M")
        );
        mailer.send(&auth, &Email { to: vec![email.clone()], subject: format!("Invitation: {title}"), body, ics: Some(ics) }).await?;
        sent += 1;
    }
    let ev = AuditEvent::new(actor, "meeting.invites", "sent").inputs(json!({ "meeting_id": meeting_id, "recipients": recipients.iter().map(|e| pseudonym(e)).collect::<Vec<_>>() })).policy_version(&d.policy_version);
    audit::append(pool, &ev).await?;
    Ok(sent)
}

/// A rubric criterion: job-related only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    pub name: String,
    pub description: String,
}

pub fn default_rubric() -> Vec<Criterion> {
    vec![
        Criterion { name: "relevant_experience".into(), description: "Experience directly relevant to the role's tasks".into() },
        Criterion { name: "skill_depth".into(), description: "Depth in the role's required skills, shown through concrete examples".into() },
        Criterion { name: "communication".into(), description: "Clarity when explaining work and decisions".into() },
        Criterion { name: "availability_fit".into(), description: "Can meet the role's stated schedule and deadlines".into() },
    ]
}

/// Questions the agent falls back on when it cannot write its own (no AI
/// key, or the model failed) and the owner gave none.
pub fn default_questions() -> Vec<String> {
    vec![
        "Tell me about a recent project that's relevant to this role, and what you did on it.".into(),
        "Walk me through how you would approach the first week of this work.".into(),
        "How do you make sure your work is delivered on time and meets what was asked?".into(),
    ]
}

/// Give a meeting that has no interviewer job yet the default one:
/// instructions from its title, no owner questions (the agent writes its
/// own), and [`default_rubric`]. Called before any join link is emailed and
/// on every join, so the agent can always be sent. Returns whether defaults
/// were stored. [`dispatch_agent`] can still replace them until the agent is
/// in the room.
pub async fn ensure_interviewer(pool: &PgPool, cfg: &PolicyConfig, actor: &Actor, meeting_id: Uuid) -> Result<bool, MeetingError> {
    let row: Option<(String, String, bool)> =
        sqlx::query_as("SELECT title, status, questions IS NOT NULL FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (title, status, has_questions) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if status != "scheduled" || has_questions {
        return Ok(false);
    }
    let c = ctx(pool, actor).await?;
    allow(cfg, &c, "dispatch_agent")?;
    let instructions = format!("Structured interview: {title}. Ask only job-related questions.");
    let (questions, rubric): (Vec<String>, _) = (vec![], default_rubric());
    let stored = sqlx::query(
        "UPDATE axom.meetings SET status = 'dispatched', instructions = $2, questions = $3, rubric = $4, callback_token_sha256 = NULL, dispatched_at = NULL \
         WHERE id = $1 AND status = 'scheduled' AND questions IS NULL",
    )
    .bind(meeting_id)
    .bind(&instructions)
    .bind(json!(questions))
    .bind(json!(rubric))
    .execute(pool)
    .await?
    .rows_affected()
        == 1;
    if stored {
        let ev = AuditEvent::new(actor, "meeting.dispatch_agent", "defaulted")
            .inputs(json!({ "meeting_id": meeting_id, "questions": questions, "questions_written_by": "ai", "rubric": rubric }))
            .reasoning("the meeting had no interviewer yet; the agent writes its questions from the role")
            .policy_version(cfg.version());
        audit::append(pool, &ev).await?;
    }
    Ok(stored)
}

/// Reject criteria that name protected characteristics.
pub fn validate_rubric(rubric: &[Criterion]) -> Result<(), MeetingError> {
    if rubric.is_empty() || rubric.len() > 12 {
        return Err(MeetingError::Invalid("rubric needs 1-12 criteria".into()));
    }
    for c in rubric {
        let text = format!("{} {}", c.name, c.description).to_lowercase();
        if let Some(p) = PROTECTED.iter().find(|p| text.split(|ch: char| !ch.is_alphanumeric()).any(|w| w.starts_with(*p))) {
            return Err(MeetingError::Invalid(format!("criterion {} refers to a protected characteristic ({p}); only job-related criteria are allowed", c.name)));
        }
        if c.name.trim().is_empty() || c.name.len() > 60 || c.description.len() > 300 {
            return Err(MeetingError::Invalid("criterion name 1-60 chars, description up to 300".into()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Dispatched {
    /// LiveKit's dispatch id; `None` while the agent waits for an invitee
    /// to join inside the join window.
    pub dispatch_id: Option<String>,
    /// Returned once, for the agent's metadata only; `None` until sent.
    pub callback_token: Option<String>,
}

/// Assign the meeting agent: store its instructions, the owner's own
/// questions (0-20; the agent writes the rest), and rubric, replacing any
/// earlier job (such as the defaults) until the agent is in the room. The
/// agent itself goes into the room when someone joins (see [`join`]), or
/// right away if an invitee already has. Sending it earlier fails: LiveKit
/// closes an empty room after ten minutes and takes the dispatch with it,
/// and a link can be open for a day.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_agent(
    pool: &PgPool,
    cfg: &PolicyConfig,
    rooms: &dyn MeetingRooms,
    actor: &Actor,
    meeting_id: Uuid,
    instructions: &str,
    questions: &[String],
    rubric: Option<Vec<Criterion>>,
    callback_base_url: &str,
) -> Result<Dispatched, MeetingError> {
    if instructions.len() > 4000 {
        return Err(MeetingError::Invalid("instructions up to 4000 characters".into()));
    }
    // The owner's own questions are optional: the agent writes its own.
    let questions: Vec<String> = questions.iter().map(|q| q.trim().to_string()).filter(|q| !q.is_empty()).collect();
    if questions.len() > 20 || questions.iter().any(|q| q.len() > 500) {
        return Err(MeetingError::Invalid("up to 20 questions of up to 500 characters".into()));
    }
    let rubric = rubric.unwrap_or_else(default_rubric);
    validate_rubric(&rubric)?;
    type MeetingRow = (String, String, DateTime<Utc>, i32, Option<DateTime<Utc>>, bool);
    let row: Option<MeetingRow> = sqlx::query_as(
        "SELECT room, status, scheduled_at, duration_minutes, first_joined_at, dispatched_at IS NOT NULL FROM axom.meetings WHERE id = $1",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await?;
    let (room, status, scheduled_at, duration, first_joined, sent_already) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if sent_already {
        return Err(MeetingError::Invalid("the interviewer is already in this meeting".into()));
    }
    if !matches!(status.as_str(), "scheduled" | "dispatched") {
        return Err(MeetingError::Invalid(format!("meeting is {status}")));
    }
    let join_deadline = link_expiry(scheduled_at);
    if Utc::now() > join_deadline {
        return Err(MeetingError::Invalid("the join window for this meeting has closed".into()));
    }
    let c = ctx(pool, actor).await?;
    allow(cfg, &c, "dispatch_agent")?;
    // The job is stored now; the callback token is minted when the agent is sent.
    let stored = sqlx::query(
        "UPDATE axom.meetings SET status = 'dispatched', instructions = $2, questions = $3, rubric = $4, callback_token_sha256 = NULL, dispatched_at = NULL \
         WHERE id = $1 AND status IN ('scheduled','dispatched') AND dispatched_at IS NULL",
    )
    .bind(meeting_id)
    .bind(instructions)
    .bind(json!(questions))
    .bind(json!(rubric))
    .execute(pool)
    .await?;
    if stored.rows_affected() == 0 {
        // An invitee joined (and the agent went in) since the read above.
        return Err(MeetingError::Invalid("the interviewer is already in this meeting".into()));
    }
    // An invitee is already waiting in the room: send the agent now.
    let sent = if first_joined.is_some() { send_agent(pool, cfg, rooms, actor, meeting_id, callback_base_url).await? } else { None };
    let ev = AuditEvent::new(actor, "meeting.dispatch_agent", if sent.is_some() { "dispatched" } else { "scheduled" })
        .inputs(json!({
            "meeting_id": meeting_id, "room": room, "duration_minutes": duration, "questions": questions, "rubric": rubric,
            "dispatch_id": sent.as_ref().map(|s| s.dispatch_id.clone()), "agent_joins": "when someone joins", "links_expire_at": join_deadline,
        }))
        .policy_version(cfg.version());
    audit::append(pool, &ev).await?;
    Ok(sent.unwrap_or(Dispatched { dispatch_id: None, callback_token: None }))
}

/// Send the assigned agent into the meeting room, once. Does nothing (and
/// returns `None`) when no agent is assigned, it was already sent, or the
/// join window has closed. The room is (re)created first: LiveKit closes
/// empty rooms after ten minutes, and the one made at scheduling time is
/// usually gone by now.
pub async fn send_agent(
    pool: &PgPool,
    cfg: &PolicyConfig,
    rooms: &dyn MeetingRooms,
    actor: &Actor,
    meeting_id: Uuid,
    callback_base_url: &str,
) -> Result<Option<Dispatched>, MeetingError> {
    let c = ctx(pool, actor).await?;
    let room_auth = allow(cfg, &c, "create_meeting")?;
    let auth = allow(cfg, &c, "dispatch_agent")?;
    let token = rand_token("cb");
    type JobRow = (String, String, Option<String>, Option<Value>, Option<Value>, DateTime<Utc>, i32);
    // Claim the send (and store the callback token's hash before the agent can use it).
    let job: Option<JobRow> = sqlx::query_as(
        "UPDATE axom.meetings SET dispatched_at = now(), callback_token_sha256 = $2 \
         WHERE id = $1 AND status = 'dispatched' AND dispatched_at IS NULL AND questions IS NOT NULL \
           AND scheduled_at + make_interval(mins => $3) >= now() \
         RETURNING room, title, instructions, questions, rubric, scheduled_at, duration_minutes",
    )
    .bind(meeting_id)
    .bind(sha(&token))
    .bind(JOIN_LATE_MINUTES as i32)
    .fetch_optional(pool)
    .await?;
    let Some((room, title, instructions, questions, rubric, scheduled_at, duration)) = job else { return Ok(None) };
    let join_deadline = link_expiry(scheduled_at);
    // The meeting title always goes with the brief: the agent writes its
    // questions from the role, and the brief may be empty.
    let instructions = match instructions.as_deref().map(str::trim).unwrap_or_default() {
        "" => format!("Meeting: {title}"),
        brief => format!("Meeting: {title}\n{brief}"),
    };
    let metadata = json!({
        "meeting_id": meeting_id, "instructions": instructions, "rubric": rubric,
        // The owner's own questions (may be none) are asked first; the agent
        // writes the rest from the role, and falls back on the defaults
        // only if it cannot.
        "questions": questions, "generate_questions": true, "fallback_questions": default_questions(),
        "disclosure": DISCLOSURE, "callback_url": format!("{}/meetings/{meeting_id}/callback", callback_base_url.trim_end_matches('/')),
        "callback_token": token,
        // The agent ends the interview at this limit, and leaves if nobody joins by the deadline.
        "max_duration_secs": i64::from(duration) * 60,
        "join_deadline": join_deadline.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
    .to_string();
    let sent = async {
        rooms.create_room(&room_auth, &room).await?;
        rooms.dispatch_agent(&auth, &room, AGENT_NAME, &metadata).await
    }
    .await;
    match sent {
        Ok(dispatch_id) => {
            let ev = AuditEvent::new(actor, "meeting.agent_sent", "sent").inputs(json!({ "meeting_id": meeting_id, "room": room, "dispatch_id": dispatch_id })).policy_version(cfg.version());
            audit::append(pool, &ev).await?;
            Ok(Some(Dispatched { dispatch_id: Some(dispatch_id), callback_token: Some(token) }))
        }
        Err(e) => {
            // Release the claim so the next join tries again.
            sqlx::query("UPDATE axom.meetings SET dispatched_at = NULL, callback_token_sha256 = NULL WHERE id = $1 AND callback_token_sha256 = $2")
                .bind(meeting_id)
                .bind(sha(&token))
                .execute(pool)
                .await?;
            let ev = AuditEvent::new(actor, "meeting.agent_sent", "failed").inputs(json!({ "meeting_id": meeting_id, "room": room })).reasoning(e.to_string()).policy_version(cfg.version());
            audit::append(pool, &ev).await?;
            Err(e.into())
        }
    }
}

/// How long a freshly sent agent has to show up in the room before a join
/// counts it as missing and sends another (it connects within seconds).
pub const AGENT_ARRIVAL_SECS: i64 = 30;

/// Make sure the interviewer is in the meeting room. Called on every join,
/// the owner's host link included:
/// - a meeting with no interviewer yet gets the default one ([`ensure_interviewer`]);
/// - an agent never sent is sent now ([`send_agent`]);
/// - an agent sent earlier that is no longer in the room (LiveKit closed the
///   room while it sat empty, or the agent left after everyone did) is
///   replaced: the earlier send is released and a new agent starts the
///   interview over, asking for consent again.
///
/// An agent sent in the last [`AGENT_ARRIVAL_SECS`] counts as on its way, so
/// a quick reload does not send two. Returns the new dispatch, if one was sent.
pub async fn ensure_agent(
    pool: &PgPool,
    cfg: &PolicyConfig,
    rooms: &dyn MeetingRooms,
    actor: &Actor,
    meeting_id: Uuid,
    callback_base_url: &str,
) -> Result<Option<Dispatched>, MeetingError> {
    ensure_interviewer(pool, cfg, actor, meeting_id).await?;
    let row: Option<(String, String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT room, status, dispatched_at FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (room, status, sent_at) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if status != "dispatched" {
        return Ok(None);
    }
    let Some(sent_at) = sent_at else {
        return send_agent(pool, cfg, rooms, actor, meeting_id, callback_base_url).await;
    };
    if Utc::now() - sent_at < Duration::seconds(AGENT_ARRIVAL_SECS) {
        return Ok(None);
    }
    let c = ctx(pool, actor).await?;
    let auth = allow(cfg, &c, "dispatch_agent")?;
    if rooms.agent_in_room(&auth, &room, AGENT_NAME).await? {
        return Ok(None);
    }
    // Release the earlier send, unless a concurrent join already did.
    let released = sqlx::query(
        "UPDATE axom.meetings SET dispatched_at = NULL, callback_token_sha256 = NULL, consent = NULL \
         WHERE id = $1 AND status = 'dispatched' AND dispatched_at = $2",
    )
    .bind(meeting_id)
    .bind(sent_at)
    .execute(pool)
    .await?
    .rows_affected()
        == 1;
    if !released {
        return Ok(None);
    }
    let ev = AuditEvent::new(actor, "meeting.agent_sent", "replacing")
        .inputs(json!({ "meeting_id": meeting_id, "room": room, "previous_sent_at": sent_at }))
        .reasoning("someone joined and the interviewer was no longer in the room; a new one starts the interview over")
        .policy_version(cfg.version());
    audit::append(pool, &ev).await?;
    send_agent(pool, cfg, rooms, actor, meeting_id, callback_base_url).await
}

#[derive(Debug, Clone, Serialize)]
pub struct JoinInfo {
    pub ws_url: String,
    pub room: String,
    pub identity: String,
    pub token: String,
    pub title: String,
    pub scheduled_at: DateTime<Utc>,
    /// The interview's time limit.
    pub duration_minutes: i32,
    /// When this join link stops working.
    pub expires_at: DateTime<Utc>,
}

/// A fresh invitee join link for an open meeting, expiring with its other
/// invitee links (used to resend an invite whose delivery failed). Returns
/// the invite token; only its hash is stored.
pub async fn new_invite_link(pool: &PgPool, meeting_id: Uuid, email: &str) -> Result<String, MeetingError> {
    let row: Option<(DateTime<Utc>, String)> = sqlx::query_as("SELECT scheduled_at, status FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (scheduled_at, status) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if !matches!(status.as_str(), "scheduled" | "dispatched") {
        return Err(MeetingError::Invalid(format!("meeting is {status}")));
    }
    Ok(insert_invite(pool, meeting_id, &email.trim().to_lowercase(), link_expiry(scheduled_at)).await?)
}

/// A fresh join link for an admin to open a meeting room from the dashboard
/// (the links made at creation are shown once and not stored). Valid for a
/// day, only while the meeting is open. Returns the invite token.
pub async fn admin_join_link(pool: &PgPool, actor: &Actor, meeting_id: Uuid, email: &str) -> Result<String, MeetingError> {
    if !actor.is_mfa_admin() {
        return Err(MeetingError::Denied("joining from the dashboard needs an MFA-verified admin".into()));
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT status FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (status,) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if !matches!(status.as_str(), "scheduled" | "dispatched") {
        return Err(MeetingError::Invalid(format!("meeting is {status}")));
    }
    let email = email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(MeetingError::Invalid("your account has no valid email address".into()));
    }
    let token = rand_token("inv");
    let mut tx = pool.begin().await?;
    // A host link: the admin can open the room before the candidate's window.
    sqlx::query("INSERT INTO axom.meeting_invites (token_sha256, meeting_id, email, expires_at, host) VALUES ($1,$2,$3,$4,true)")
        .bind(sha(&token))
        .bind(meeting_id)
        .bind(&email)
        .bind(Utc::now() + Duration::days(1))
        .execute(&mut *tx)
        .await?;
    let ev = AuditEvent::new(actor, "meeting.admin_join_link", "created").inputs(json!({ "meeting_id": meeting_id, "admin": pseudonym(&email) }));
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(token)
}

/// Exchange a join-link token for a short-lived LiveKit participant token.
/// Every join, the owner's host link included, also makes sure the
/// interviewer is in the room ([`ensure_agent`]).
pub async fn join(pool: &PgPool, cfg: &PolicyConfig, rooms: &dyn MeetingRooms, ws_url: &str, callback_base_url: &str, invite_token: &str) -> Result<JoinInfo, MeetingError> {
    type JoinRow = (Uuid, String, DateTime<Utc>, String, String, String, DateTime<Utc>, i32, bool, Option<DateTime<Utc>>);
    let row: Option<JoinRow> = sqlx::query_as(
        "SELECT m.id, i.email, i.expires_at, m.room, m.status, m.title, m.scheduled_at, m.duration_minutes, i.host, m.first_joined_at \
         FROM axom.meeting_invites i JOIN axom.meetings m ON m.id = i.meeting_id WHERE i.token_sha256 = $1",
    )
    .bind(sha(invite_token))
    .fetch_optional(pool)
    .await?;
    let Some((meeting_id, email, expires, room, status, title, scheduled_at, duration_minutes, host, first_joined)) = row else { return Err(MeetingError::BadToken) };
    let now = Utc::now();
    if matches!(status.as_str(), "completed" | "cancelled" | "consent_declined") {
        return Err(MeetingError::Denied("this meeting has ended, so its link is no longer valid".into()));
    }
    if now > expires || status == "expired" {
        return Err(MeetingError::Denied(format!("this interview link expired at {}; ask the hiring team for a new time", utc(expires))));
    }
    // Once an invitee has joined, invitee links last only the interview's time limit.
    let duration = Duration::minutes(i64::from(duration_minutes));
    if let Some(ran_out) = first_joined.map(|t| t + duration).filter(|end| !host && now > *end) {
        return Err(MeetingError::Denied(format!(
            "this interview's {duration_minutes} minutes ran out at {}, so its link is no longer valid",
            utc(ran_out)
        )));
    }
    let c = ctx(pool, &Actor::System).await?;
    let auth = allow(cfg, &c, "join_meeting")?;
    let token = rooms.participant_token(&auth, &room, &email).await?;
    let mut expires_at = expires;
    if !host {
        // The first invitee to join starts the clock; later joins keep it.
        let (started, first): (DateTime<Utc>, bool) = sqlx::query_as(
            "UPDATE axom.meetings SET first_joined_at = COALESCE(first_joined_at, $2) WHERE id = $1 RETURNING first_joined_at, first_joined_at = $2",
        )
        .bind(meeting_id)
        .bind(now)
        .fetch_one(pool)
        .await?;
        if first && first_joined.is_none() {
            let ev = AuditEvent::new(&Actor::System, "meeting.first_join", "joined")
                .inputs(json!({ "meeting_id": meeting_id, "invitee": pseudonym(&email), "links_expire_at": started + duration }))
                .policy_version(cfg.version());
            audit::append(pool, &ev).await?;
        }
        expires_at = expires.min(started + duration);
    }
    // Whoever joins, the interviewer is there: sent now if it never was, and
    // sent again if it has left. A failure here must not lock the
    // participant out; it is audited and the next join tries again.
    if let Err(e) = ensure_agent(pool, cfg, rooms, &Actor::System, meeting_id, callback_base_url).await {
        tracing::error!(%meeting_id, error = %e, "sending the meeting agent failed");
    }
    Ok(JoinInfo { ws_url: ws_url.into(), room, identity: email, token, title, scheduled_at, duration_minutes, expires_at })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: String,
    pub text: String,
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
}

/// Events the meeting agent posts back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    Consent { granted: bool, statement: String, at: DateTime<Utc> },
    Completed { transcript: Vec<Turn>, summary: String, scores: std::collections::BTreeMap<String, i32> },
}

/// The agent's per-meeting token is right and the meeting is running.
pub async fn check_agent_token(pool: &PgPool, meeting_id: Uuid, token: &str) -> Result<(), MeetingError> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT callback_token_sha256, status FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (expected, status) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if token.is_empty() || expected.as_deref() != Some(sha(token).as_str()) {
        return Err(MeetingError::BadToken);
    }
    if status != "dispatched" {
        return Err(MeetingError::Invalid(format!("meeting is {status}")));
    }
    Ok(())
}

/// The meeting is over: every join link for it (invitee and host) stops
/// working now. Returns how many links were cut short.
async fn expire_links<'e, E: sqlx::PgExecutor<'e>>(ex: E, meeting_id: Uuid) -> Result<u64, sqlx::Error> {
    let r = sqlx::query("UPDATE axom.meeting_invites SET expires_at = now() WHERE meeting_id = $1 AND expires_at > now()")
        .bind(meeting_id)
        .execute(ex)
        .await?;
    Ok(r.rows_affected())
}

pub async fn record_callback(pool: &PgPool, meeting_id: Uuid, token: &str, event: AgentEvent) -> Result<String, MeetingError> {
    type CallbackRow = (Option<String>, String, Option<Value>, Option<Value>);
    let row: Option<CallbackRow> =
        sqlx::query_as("SELECT callback_token_sha256, status, consent, rubric FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (expected, status, consent, rubric) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    if expected.as_deref() != Some(sha(token).as_str()) {
        return Err(MeetingError::BadToken);
    }
    if status != "dispatched" {
        return Err(MeetingError::Invalid(format!("meeting is {status}")));
    }
    let actor = Actor::agent(AGENT_NAME);
    match event {
        AgentEvent::Consent { granted, statement, at } => {
            if consent.is_some() {
                return Err(MeetingError::Invalid("consent already recorded".into()));
            }
            let record = json!({ "granted": granted, "statement": Untrusted::new("participant", statement).raw(), "at": at, "disclosure": DISCLOSURE });
            let new_status = if granted { "dispatched" } else { "consent_declined" };
            sqlx::query("UPDATE axom.meetings SET consent = $2, status = $3, completed_at = CASE WHEN $4 THEN NULL ELSE now() END WHERE id = $1")
                .bind(meeting_id)
                .bind(&record)
                .bind(new_status)
                .bind(granted)
                .execute(pool)
                .await?;
            // Without consent the call is over: its links stop working.
            let links_expired = if granted { 0 } else { expire_links(pool, meeting_id).await? };
            let ev = AuditEvent::new(&actor, "meeting.consent", if granted { "granted" } else { "declined" })
                .inputs(json!({ "meeting_id": meeting_id, "granted": granted, "at": at, "links_expired": links_expired }));
            audit::append(pool, &ev).await?;
            Ok(new_status.into())
        }
        AgentEvent::Completed { transcript, summary, scores } => {
            let granted = consent.as_ref().and_then(|c| c.get("granted")).and_then(Value::as_bool) == Some(true);
            if !granted {
                return Err(MeetingError::Denied("no recorded consent: transcript rejected".into()));
            }
            let criteria: Vec<String> = rubric
                .and_then(|r| serde_json::from_value::<Vec<Criterion>>(r).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|c| c.name)
                .collect();
            for (k, v) in &scores {
                if !criteria.contains(k) {
                    return Err(MeetingError::Invalid(format!("score for {k} is not a rubric criterion")));
                }
                if !(1..=5).contains(v) {
                    return Err(MeetingError::Invalid(format!("score {v} for {k} must be 1-5")));
                }
            }
            if transcript.len() > 2000 || summary.len() > 20_000 {
                return Err(MeetingError::Invalid("transcript or summary too large".into()));
            }
            sqlx::query("UPDATE axom.meetings SET transcript = $2, summary = $3, scores = $4, status = 'completed', completed_at = now() WHERE id = $1")
                .bind(meeting_id)
                .bind(json!(transcript))
                .bind(&summary)
                .bind(json!(scores))
                .execute(pool)
                .await?;
            let links_expired = expire_links(pool, meeting_id).await?;
            let ev = AuditEvent::new(&actor, "meeting.completed", "completed")
                .inputs(json!({ "meeting_id": meeting_id, "turns": transcript.len(), "scores": scores, "links_expired": links_expired }));
            audit::append(pool, &ev).await?;
            Ok("completed".into())
        }
    }
}

/// Recipients `send_summary` may reach: invitees, the owner, and recipients
/// the owner added in the dashboard.
pub async fn allowed_recipients(pool: &PgPool, meeting_id: Uuid, owner_email: &str) -> Result<Vec<String>, MeetingError> {
    let row: Option<(Vec<String>, Vec<String>)> =
        sqlx::query_as("SELECT invitees, extra_recipients FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (mut inv, extra) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    inv.extend(extra);
    if !owner_email.is_empty() {
        inv.push(owner_email.to_lowercase());
    }
    Ok(inv)
}

#[derive(Debug, Clone, Serialize)]
pub struct SummarySent {
    pub recipients: Vec<String>,
    pub crm_note_id: Option<String>,
}

/// Send the meeting summary. Every recipient must be on the allow-list; the
/// recipient list comes from the caller (dashboard, MCP), never from the
/// transcript or summary text.
#[allow(clippy::too_many_arguments)]
pub async fn send_summary(
    pool: &PgPool,
    cfg: &PolicyConfig,
    notify: &NotifyConfig,
    mailer: &dyn Mailer,
    crm: &dyn Crm,
    actor: &Actor,
    meeting_id: Uuid,
    recipients: &[String],
) -> Result<SummarySent, MeetingError> {
    let recipients: Vec<String> = recipients.iter().map(|r| r.trim().to_lowercase()).collect();
    if recipients.is_empty() || recipients.len() > 25 || !recipients.iter().all(|r| valid_email(r)) {
        return Err(MeetingError::Invalid("1-25 valid recipient emails required".into()));
    }
    let row: Option<(String, Option<String>, String, Vec<String>)> =
        sqlx::query_as("SELECT title, summary, status, invitees FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_optional(pool).await?;
    let (title, summary, status, invitees) = row.ok_or(MeetingError::NotFound(meeting_id))?;
    let Some(summary) = summary.filter(|_| status == "completed") else {
        return Err(MeetingError::Invalid("meeting has no summary yet".into()));
    };
    let allowed = allowed_recipients(pool, meeting_id, &notify.owner_email).await?;
    let mut c = ctx(pool, actor).await?;
    c.actor = actor.clone();
    let d = policy::evaluate(cfg, &Action::SendExternalMessage { recipients: recipients.clone(), allowed_recipients: allowed }, &c);
    let Some(auth) = Authorized::from_decision("send_summary", &d) else {
        let ev = AuditEvent::new(actor, "meeting.send_summary", "denied")
            .inputs(json!({ "meeting_id": meeting_id, "recipients": recipients }))
            .reasoning(&d.reason)
            .policy_version(&d.policy_version);
        audit::append(pool, &ev).await?;
        return Err(MeetingError::Denied(d.reason));
    };
    let body = format!("Summary of \"{title}\"\n\n{summary}\n\n(Generated by Combox's AI interviewer from the recorded transcript.)");
    let email = Email { to: recipients.clone(), subject: format!("Meeting summary: {title}"), body, ics: None };
    mailer.send(&auth, &email).await?;

    // Save as a CRM note on the candidate (the first invitee).
    let crm_auth = Authorized::from_decision("crm_write", &d);
    let mut note_id = None;
    if let (Some(a), Some(candidate)) = (crm_auth, invitees.first()) {
        let contact_id = crm.upsert_contact(&a, &Contact { name: candidate.clone(), email: candidate.clone(), company: None }).await?;
        note_id = Some(crm.add_note(&a, &contact_id, &format!("Meeting summary: {title}\n\n{summary}")).await?);
    }
    sqlx::query("UPDATE axom.meetings SET summary_sent_at = now() WHERE id = $1").bind(meeting_id).execute(pool).await?;
    let ev = AuditEvent::new(actor, "meeting.send_summary", "sent")
        .inputs(json!({ "meeting_id": meeting_id, "recipients": recipients, "crm_note_id": note_id }))
        .policy_version(&d.policy_version);
    audit::append(pool, &ev).await?;
    Ok(SummarySent { recipients, crm_note_id: note_id })
}

/// The owner adds an extra summary recipient in the dashboard (MFA admin).
pub async fn add_recipient(pool: &PgPool, actor: &Actor, meeting_id: Uuid, email: &str) -> Result<(), MeetingError> {
    if !actor.is_mfa_admin() {
        return Err(MeetingError::Denied("only the owner (MFA-verified admin) can add summary recipients".into()));
    }
    if !valid_email(email) {
        return Err(MeetingError::Invalid("invalid email".into()));
    }
    let n = sqlx::query("UPDATE axom.meetings SET extra_recipients = array_append(extra_recipients, $2) WHERE id = $1 AND NOT ($2 = ANY(extra_recipients))")
        .bind(meeting_id)
        .bind(email.trim().to_lowercase())
        .execute(pool)
        .await?
        .rows_affected();
    let ev = AuditEvent::new(actor, "meeting.add_recipient", if n == 1 { "added" } else { "unchanged" }).inputs(json!({ "meeting_id": meeting_id, "email": email }));
    audit::append(pool, &ev).await?;
    Ok(())
}

/// Mark meetings that can no longer happen as `expired`: still scheduled or
/// dispatched after the latest possible end (link expiry + time limit +
/// grace). Their rooms are closed and the owner is notified.
pub async fn expire_meetings(pool: &PgPool, cfg: &PolicyConfig, rooms: &dyn MeetingRooms, now: DateTime<Utc>) -> Result<Vec<Uuid>, MeetingError> {
    let expired: Vec<(Uuid, String, DateTime<Utc>, bool)> = sqlx::query_as(
        "UPDATE axom.meetings SET status = 'expired', expired_at = $1 \
         WHERE status IN ('scheduled','dispatched') \
           AND scheduled_at + make_interval(mins => $2 + duration_minutes) < $1 \
         RETURNING id, room, scheduled_at, consent IS NOT NULL",
    )
    .bind(now)
    .bind((JOIN_LATE_MINUTES + EXPIRE_GRACE_MINUTES) as i32)
    .fetch_all(pool)
    .await?;
    if expired.is_empty() {
        return Ok(vec![]);
    }
    let c = ctx(pool, &Actor::System).await?;
    let end = allow(cfg, &c, "end_meeting").ok();
    let mut ids = Vec::new();
    for (id, room, scheduled_at, joined) in expired {
        let closed = match &end {
            Some(auth) => rooms.end_room(auth, &room).await.map_err(|e| e.to_string()),
            None => Err("end_meeting not allowed".into()),
        };
        let reason = if joined { "the interview started but no results arrived before the time limit" } else { "nobody joined before the link expired" };
        let mut tx = pool.begin().await?;
        expire_links(&mut *tx, id).await?;
        let ev = AuditEvent::new(&Actor::System, "meeting.expire", "expired")
            .inputs(json!({ "meeting_id": id, "room": room, "scheduled_at": scheduled_at, "room_closed": closed.is_ok(), "room_error": closed.err() }))
            .reasoning(reason)
            .policy_version(cfg.version());
        let audit_id = audit::append(&mut *tx, &ev).await?;
        // No title: it can hold the candidate's name, and notifications are kept.
        crate::notify::enqueue(
            &mut *tx,
            "meeting_expired",
            "Interview expired",
            json!({ "meeting_id": id, "scheduled_at": scheduled_at, "reason": reason }),
            Some(audit_id),
        )
        .await?;
        tx.commit().await?;
        ids.push(id);
    }
    Ok(ids)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReminderSent {
    pub meeting_id: Uuid,
    pub slot: &'static str,
    pub recipients: Vec<String>,
    pub failed: Vec<String>,
}

/// The latest reminder slot whose time has come, if any.
pub fn due_slot(scheduled_at: DateTime<Utc>, now: DateTime<Utc>) -> Option<(&'static str, i64)> {
    if now >= scheduled_at {
        return None;
    }
    REMINDERS.iter().copied().rev().find(|(_, mins)| now >= scheduled_at - Duration::minutes(*mins))
}

/// Remind invitees of upcoming interviews. Each slot is sent at most once
/// per meeting; a slot that had already passed when the meeting was booked
/// is skipped, and so is any earlier slot once a later one is due. Each
/// reminder carries a fresh join link (only token hashes are stored) and,
/// by email, a calendar invite with alarms. Upwork applicants without a
/// shared email are reminded through the marketplace.
#[allow(clippy::too_many_arguments)]
pub async fn send_due_reminders(
    pool: &PgPool,
    cfg: &PolicyConfig,
    mailer: &dyn Mailer,
    market: &dyn FreelanceMarketplace,
    meet_web_url: &str,
    organizer: &str,
    now: DateTime<Utc>,
) -> Result<Vec<ReminderSent>, MeetingError> {
    type Upcoming = (Uuid, String, DateTime<Utc>, DateTime<Utc>, Vec<String>, i32);
    let upcoming: Vec<Upcoming> = sqlx::query_as(
        "SELECT id, title, scheduled_at, created_at, invitees, duration_minutes FROM axom.meetings \
         WHERE status IN ('scheduled','dispatched') AND scheduled_at > $1 AND scheduled_at <= $1 + make_interval(mins => $2)",
    )
    .bind(now)
    .bind(REMINDERS[0].1 as i32)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (id, title, scheduled_at, created_at, invitees, duration) in upcoming {
        let Some((slot, mins)) = due_slot(scheduled_at, now) else { continue };
        // Claim this slot and retire earlier ones; skip if already handled.
        // A slot whose time was before the booking is recorded as skipped.
        let on_time = scheduled_at - Duration::minutes(mins) >= created_at - Duration::minutes(1);
        let mut tx = pool.begin().await?;
        for (earlier, _) in REMINDERS.iter().filter(|(_, m)| *m > mins) {
            sqlx::query("INSERT INTO axom.meeting_reminders (meeting_id, slot, sent) VALUES ($1,$2,false) ON CONFLICT DO NOTHING")
                .bind(id)
                .bind(earlier)
                .execute(&mut *tx)
                .await?;
        }
        let claimed = sqlx::query("INSERT INTO axom.meeting_reminders (meeting_id, slot, sent) VALUES ($1,$2,false) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(slot)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        if !claimed || !on_time {
            continue;
        }

        // The reminder carries a working link, so the interviewer needs questions.
        ensure_interviewer(pool, cfg, &Actor::System, id).await?;
        let expires_at = link_expiry(scheduled_at);
        let c = ctx(pool, &Actor::System).await?;
        let when = match mins {
            m if m >= 24 * 60 => "tomorrow".to_string(),
            m if m >= 60 => "in 1 hour".to_string(),
            m => format!("in {m} minutes"),
        };
        let (mut sent, mut failed) = (Vec::new(), Vec::new());
        // Erased invitees ('[deleted]') are skipped.
        for email in invitees.iter().filter(|e| valid_email(e)) {
            let token = insert_invite(pool, id, email, expires_at).await?;
            let join = format!("{}/join?t={token}", meet_web_url.trim_end_matches('/'));
            let text = format!(
                "Reminder: your interview \"{title}\" starts {when}, at {}. It lasts {duration} minutes and is with an AI interviewer, \
                 which will introduce itself and ask for your consent before anything is recorded.\n\n\
                 Join here: {join}\n\nYou can join any time until {}; the interview starts when you join.",
                utc(scheduled_at),
                utc(expires_at)
            );
            let d = policy::evaluate(cfg, &Action::SendExternalMessage { recipients: vec![email.clone()], allowed_recipients: invitees.clone() }, &c);
            let result = match Authorized::from_decision("send_external_message", &d) {
                None => Err(d.reason.clone()),
                Some(auth) if email.ends_with(PLACEHOLDER_DOMAIN) => {
                    let ext: Option<(String,)> =
                        sqlx::query_as("SELECT external_applicant_id FROM axom.candidates WHERE meeting_id = $1").bind(id).fetch_optional(pool).await?;
                    match ext {
                        Some((ext,)) => market.message_candidate(&auth, &ext, &text).await.map_err(|e| e.to_string()),
                        None => Err("no marketplace contact for this invitee".into()),
                    }
                }
                Some(auth) => {
                    let ics = axom_tools::adapters::smtp::ics_invite(&format!("{id}@axom"), &title, scheduled_at, i64::from(duration), organizer, &join);
                    let mail = Email { to: vec![email.clone()], subject: format!("Reminder: {title} starts {when}"), body: text, ics: Some(ics) };
                    mailer.send(&auth, &mail).await.map(|_| ()).map_err(|e| e.to_string())
                }
            };
            match result {
                Ok(()) => sent.push(email.clone()),
                Err(e) => {
                    tracing::warn!(meeting_id = %id, slot, error = %e, "interview reminder not sent");
                    failed.push(email.clone());
                }
            }
        }
        sqlx::query("UPDATE axom.meeting_reminders SET sent = $3, sent_count = $4 WHERE meeting_id = $1 AND slot = $2")
            .bind(id)
            .bind(slot)
            .bind(!sent.is_empty())
            .bind(sent.len() as i32)
            .execute(pool)
            .await?;
        let ev = AuditEvent::new(&Actor::System, "meeting.reminder", if failed.is_empty() { "sent" } else if sent.is_empty() { "failed" } else { "partial" })
            // Pseudonymous references only (append-only audit log).
            .inputs(json!({
                "meeting_id": id, "slot": slot, "scheduled_at": scheduled_at,
                "sent": sent.iter().map(|e| pseudonym(e)).collect::<Vec<_>>(),
                "failed": failed.iter().map(|e| pseudonym(e)).collect::<Vec<_>>(),
            }))
            .policy_version(cfg.version());
        audit::append(pool, &ev).await?;
        out.push(ReminderSent { meeting_id: id, slot, recipients: sent, failed });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubric_rejects_protected_characteristics() {
        assert!(validate_rubric(&default_rubric()).is_ok());
        for bad in ["age", "Gender fit", "nationality", "religious_alignment", "disability_status", "native accent"] {
            let r = vec![Criterion { name: bad.into(), description: "x".into() }];
            assert!(validate_rubric(&r).is_err(), "{bad} should be rejected");
        }
        // Job-related words that merely contain substrings are fine.
        let ok = vec![Criterion { name: "management_experience".into(), description: "Has managed delivery teams".into() }];
        assert!(validate_rubric(&ok).is_ok());
    }

    #[test]
    fn link_expiry_and_reminder_slots() {
        let start = "2026-11-20T15:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(link_expiry(start) - start, Duration::hours(24));
        let at = |m: i64| start - Duration::minutes(m);
        assert_eq!(due_slot(start, at(25 * 60)), None);
        assert_eq!(due_slot(start, at(24 * 60)).map(|s| s.0), Some("24h"));
        assert_eq!(due_slot(start, at(90)).map(|s| s.0), Some("24h"));
        assert_eq!(due_slot(start, at(60)).map(|s| s.0), Some("1h"));
        assert_eq!(due_slot(start, at(5)).map(|s| s.0), Some("10m"));
        assert_eq!(due_slot(start, start), None, "no reminders once it has started");
    }
}
