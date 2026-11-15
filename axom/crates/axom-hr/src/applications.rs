//! Where each jobs-board application stands, for the admin dashboard: a
//! progress stage (applied → screened → interview → scored → contract →
//! hired), a one-line status, and, for anyone out of the running, the reason.
//!
//! Nothing here changes state; it reads the application, its candidate row,
//! the interview meeting and the hiring audit trail. What applicants typed
//! (name, skills, cover letter, consent statement) is shown as plain data.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::hiring::{skill_matches, SkillMatch, MIN_RUBRIC_SCORE, SHORTLIST_SIZE};
use crate::meetings;

/// Everything needed to place one application in the pipeline.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    pub id: Uuid,
    pub job_id: Uuid,
    pub job_title: String,
    pub job_skills: Vec<String>,
    pub job_status: String,
    pub name: String,
    pub email: String,
    pub skills: Vec<String>,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub posting_status: Option<String>,
    pub candidate_id: Option<Uuid>,
    pub status: Option<String>,
    pub shortlist_score: Option<f64>,
    pub rubric_score: Option<f64>,
    pub screened_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub meeting_id: Option<Uuid>,
    pub meeting_status: Option<String>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub first_joined_at: Option<DateTime<Utc>>,
    pub consent: Option<Value>,
    pub scores: Option<Value>,
    pub meeting_completed_at: Option<DateTime<Utc>>,
    pub expired_at: Option<DateTime<Utc>>,
    pub cv_filename: Option<String>,
    pub cv_size: Option<i32>,
    /// Candidates of this posting in the interview pipeline (incl. this one).
    pub in_pipeline: i64,
    /// Another candidate of this posting holds the contract or the job.
    pub other_offer: Option<String>,
    /// The latest hiring audit event about this candidate.
    pub last_action: Option<String>,
    pub last_outcome: Option<String>,
    pub last_reason: Option<String>,
}

const SELECT: &str = "
SELECT a.id, a.job_id, j.title AS job_title, j.skills AS job_skills, j.status AS job_status,
       a.name, a.email, a.skills, a.message, a.created_at,
       p.status AS posting_status,
       c.id AS candidate_id, c.status, c.shortlist_score, c.rubric_score, c.created_at AS screened_at, c.updated_at,
       m.id AS meeting_id, m.status AS meeting_status, m.scheduled_at, m.first_joined_at, m.consent, m.scores,
       m.completed_at AS meeting_completed_at, m.expired_at,
       f.filename AS cv_filename, f.size_bytes AS cv_size,
       COALESCE((SELECT count(*) FROM axom.candidates o WHERE o.job_posting_id = p.id
                  AND o.status IN ('shortlisted','invited','interviewed','contract_sent','hired')), 0) AS in_pipeline,
       (SELECT o.status FROM axom.candidates o WHERE o.job_posting_id = p.id AND o.id IS DISTINCT FROM c.id
                  AND o.status IN ('contract_sent','hired') ORDER BY o.status = 'hired' DESC LIMIT 1) AS other_offer,
       ev.action AS last_action, ev.outcome AS last_outcome, ev.reasoning_summary AS last_reason
FROM axom.board_applications a
JOIN axom.board_jobs j ON j.id = a.job_id
LEFT JOIN axom.job_postings p ON p.marketplace = 'axom_board' AND p.external_job_id = a.job_id::text
LEFT JOIN axom.candidates c ON c.job_posting_id = p.id AND c.external_applicant_id = a.id::text
LEFT JOIN axom.meetings m ON m.id = c.meeting_id
LEFT JOIN axom.board_application_cvs f ON f.application_id = a.id
LEFT JOIN LATERAL (
    SELECT l.action, l.outcome, l.reasoning_summary FROM axom.audit_log l
    WHERE c.id IS NOT NULL
      AND l.action IN ('hiring.invite','hiring.score','hiring.send_contract','hiring.no_show','hiring.complete','hiring.decline')
      AND l.inputs->>'candidate_id' = c.id::text
    ORDER BY l.seq DESC LIMIT 1
) ev ON true
WHERE a.name <> '[deleted]'";

/// Pipeline steps, in order; `Assessment::step` indexes this.
pub const STEPS: [&str; 6] = ["applied", "screened", "interview", "scored", "contract", "hired"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assessment {
    /// Machine name of where it stands, e.g. `waiting_for_slot`.
    pub stage: &'static str,
    /// Index into [`STEPS`] of the step it is on (or stopped at).
    pub step: usize,
    /// `working` (Combox is acting now), `waiting` (on the candidate, a slot
    /// or the next pass), `done` (hired) or `stopped` (out of the running).
    pub state: &'static str,
    /// `in_progress`, `hired`, `not_qualified`, `not_selected`, `withdrew` or `closed`.
    pub outcome: &'static str,
    /// What is happening, in a few words.
    pub headline: String,
    /// More detail: what comes next, or why it stopped.
    pub detail: String,
}

fn a(stage: &'static str, step: usize, state: &'static str, outcome: &'static str, headline: impl Into<String>, detail: impl Into<String>) -> Assessment {
    Assessment { stage, step, state, outcome, headline: headline.into(), detail: detail.into() }
}

fn words(skills: &[String]) -> String {
    skills.iter().map(|s| s.replace('_', " ")).collect::<Vec<_>>().join(", ")
}

fn pct(score: f64) -> String {
    format!("{:.0}%", (score * 100.0).clamp(0.0, 100.0))
}

/// The criterion with the lowest interview score, as "communication 2/5".
fn weakest(scores: &Option<Value>) -> Option<String> {
    let o = scores.as_ref()?.as_object()?;
    let (k, v) = o.iter().filter_map(|(k, v)| v.as_f64().map(|v| (k, v))).min_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))?;
    Some(format!("{} {}/5", k.replace('_', " "), v))
}

fn job_open(r: &Row) -> bool {
    r.posting_status.as_deref().unwrap_or("open") == "open" && r.job_status == "open"
}

/// Where the application stands and why. Pure, so it can be tested.
pub fn assess(r: &Row, now: DateTime<Utc>) -> Assessment {
    let matches = skill_matches(&r.job_skills, &r.skills, &r.message);
    let missing: Vec<String> = matches.iter().filter(|(_, m)| *m == SkillMatch::Missing).map(|(s, _)| s.clone()).collect();
    let score = r.shortlist_score.unwrap_or_else(|| crate::hiring::shortlist_score(&r.job_skills, &r.skills, &r.message));
    let status = r.status.as_deref().unwrap_or("");
    let filled = r.other_offer.as_deref() == Some("hired");
    let closed_why = if filled { "The position was filled by another candidate." } else { "The job was closed." };

    match status {
        // Not looked at yet: the hiring pass picks up new applicants every minute.
        "" if !job_open(r) => a("closed", 1, "stopped", "closed", "Job closed before review", closed_why),
        "" => a("awaiting_screening", 1, "waiting", "in_progress", "Waiting to be screened", "Combox checks new applicants against the job's skills within a minute or two."),
        "applied" if score <= 0.0 => a(
            "not_qualified_skills",
            1,
            "stopped",
            "not_qualified",
            "Not qualified: no skill match",
            format!(
                "None of the required skills ({}) match what they listed ({}).",
                words(&r.job_skills),
                if r.skills.is_empty() { "no skills".to_string() } else { words(&r.skills) }
            ),
        ),
        "applied" if !job_open(r) => a("closed", 1, "stopped", "not_selected", "Not reached before the job closed", format!("{} They matched {} of the skills.", closed_why, pct(score))),
        "applied" => a(
            "waiting_for_slot",
            1,
            "waiting",
            "in_progress",
            format!("Qualified ({} skill match), in the queue", pct(score)),
            format!(
                "Up to {SHORTLIST_SIZE} candidates are interviewed at a time and {} {} in progress now; stronger skill matches go first.{}",
                r.in_pipeline,
                if r.in_pipeline == 1 { "is" } else { "are" },
                if missing.is_empty() { String::new() } else { format!(" Missing: {}.", words(&missing)) }
            ),
        ),
        "shortlisted" if r.last_action.as_deref() == Some("hiring.invite") && r.last_outcome.as_deref() == Some("send_failed") => a(
            "invite_failed",
            2,
            "working",
            "in_progress",
            "Invite didn't send, retrying",
            format!("The interview is booked but the email failed; the next pass (within a minute) retries. {}", r.last_reason.clone().unwrap_or_default()),
        ),
        "shortlisted" => a("creating_interview", 2, "working", "in_progress", "Shortlisted, creating the interview", "Booking the AI interview and emailing the join link."),
        "invited" => match r.meeting_status.as_deref() {
            Some("completed") | Some("consent_declined") => a("scoring", 3, "working", "in_progress", "Interview finished, scoring", "The rubric scores are recorded on the next pass."),
            _ if r.first_joined_at.is_some() || r.consent.is_some() => a("interviewing", 2, "working", "in_progress", "Interview in progress", "They joined the AI interview."),
            _ => {
                let expires = r.scheduled_at.map(meetings::link_expiry);
                let left = expires.map(|e| e - now);
                let detail = match left {
                    Some(d) if d.num_minutes() > 0 => format!(
                        "The join link was emailed. It works until {} ({} left); if they don't join by then, the slot goes to the next applicant.",
                        expires.unwrap().format("%b %-d, %H:%M UTC"),
                        if d.num_hours() >= 1 { format!("{}h", d.num_hours()) } else { format!("{}m", d.num_minutes()) }
                    ),
                    _ => "The join link has expired; they'll be marked as a no-show on the next pass.".to_string(),
                };
                a("invited", 2, "waiting", "in_progress", "Invited, waiting for them to join", detail)
            }
        },
        "interviewed" => {
            let s = r.rubric_score.unwrap_or(0.0);
            if s < MIN_RUBRIC_SCORE {
                a(
                    "below_bar",
                    3,
                    "stopped",
                    "not_qualified",
                    format!("Not qualified: interview {s:.1}/5"),
                    format!(
                        "Their interview score {s:.2} is below the {MIN_RUBRIC_SCORE} needed for a contract.{}",
                        weakest(&r.scores).map(|w| format!(" Weakest: {w}.")).unwrap_or_default()
                    ),
                )
            } else if r.other_offer.is_some() {
                a(
                    "runner_up",
                    3,
                    "waiting",
                    "in_progress",
                    format!("Passed ({s:.1}/5), runner-up"),
                    "Another candidate got the contract first. If they decline, this candidate is next in line by score.",
                )
            } else {
                a("passed", 3, "working", "in_progress", format!("Passed ({s:.1}/5), preparing contract"), "The best candidate at or above the bar gets the contract on the next pass.")
            }
        }
        "contract_sent" => a("contract_sent", 4, "waiting", "in_progress", "Contract sent, waiting for signature", "They were emailed a contract from your approved template. They're activated as soon as they sign."),
        "hired" => a("hired", 5, "done", "hired", "Hired", "Contract signed. They have their briefs access."),
        "declined" => {
            if r.meeting_status.as_deref() == Some("consent_declined") || (r.last_action.as_deref() == Some("hiring.decline") && r.rubric_score.is_none()) {
                a("declined_recording", 2, "stopped", "withdrew", "Declined the interview", "They said no when the AI interviewer asked for consent to record, so the interview ended there.")
            } else {
                a("declined_contract", 4, "stopped", "withdrew", "Declined the contract", "They passed the interview but declined the contract.")
            }
        }
        "rejected" => {
            if r.last_action.as_deref() == Some("hiring.no_show") {
                a("no_show", 2, "stopped", "not_selected", "Missed the interview", "They never joined the AI interview before the link expired, so the slot went to the next applicant.")
            } else if filled || r.other_offer.is_some() || !job_open(r) {
                let step = if r.rubric_score.is_some() { 3 } else if r.meeting_id.is_some() { 2 } else { 1 };
                let detail = match r.rubric_score {
                    Some(s) if s >= MIN_RUBRIC_SCORE => format!("{closed_why} They passed the interview ({s:.1}/5) but scored lower."),
                    Some(s) => format!("{closed_why} Their interview score was {s:.1}/5, below the {MIN_RUBRIC_SCORE} bar."),
                    None => closed_why.to_string(),
                };
                a("position_filled", step, "stopped", "not_selected", if filled { "Not selected: position filled" } else { "Not selected: job closed" }, detail)
            } else {
                a("rejected", 1, "stopped", "not_selected", "Turned down", "Their data was removed under the retention policy or a deletion request.")
            }
        }
        other => a("unknown", 1, "waiting", "in_progress", other.replace('_', " "), ""),
    }
}

/// One application as the dashboard lists it.
pub fn view(r: &Row, now: DateTime<Utc>) -> Value {
    let assessment = assess(r, now);
    json!({
        "id": r.id, "job_id": r.job_id, "job_title": r.job_title, "name": r.name, "email": r.email, "skills": r.skills,
        "untrusted_message": r.message, "created_at": r.created_at, "updated_at": r.updated_at.unwrap_or(r.created_at),
        "candidate_id": r.candidate_id, "status": r.status.clone().unwrap_or_else(|| "applied".into()),
        "shortlist_score": r.shortlist_score, "interview_score": r.rubric_score,
        "cv": r.cv_filename.as_ref().map(|n| json!({ "filename": n, "size_bytes": r.cv_size })),
        "progress": assessment,
    })
}

/// Every application, newest first.
pub async fn list(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(&format!("{SELECT} ORDER BY a.created_at DESC LIMIT 500")).fetch_all(pool).await?;
    let now = Utc::now();
    Ok(rows.iter().map(|r| view(r, now)).collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub at: DateTime<Utc>,
    /// `info`, `good`, `warn` or `bad`.
    pub tone: &'static str,
    pub title: String,
    pub detail: String,
}

fn ev(at: DateTime<Utc>, tone: &'static str, title: impl Into<String>, detail: impl Into<String>) -> Event {
    Event { at, tone, title: title.into(), detail: detail.into() }
}

/// One application with its full progress log, oldest first, plus the
/// skill match and interview scores. `None` if there is no such application.
pub async fn detail(pool: &PgPool, id: Uuid) -> Result<Option<Value>, sqlx::Error> {
    let Some(r): Option<Row> = sqlx::query_as(&format!("{SELECT} AND a.id = $1")).bind(id).fetch_optional(pool).await? else {
        return Ok(None);
    };
    let now = Utc::now();
    let matches = skill_matches(&r.job_skills, &r.skills, &r.message);
    let mut log = vec![ev(
        r.created_at,
        "info",
        format!("Applied for {}", r.job_title),
        if r.cv_filename.is_some() { "With a CV." } else { "No CV attached." },
    )];

    if let (Some(cid), Some(at)) = (r.candidate_id, r.screened_at) {
        let score = r.shortlist_score.unwrap_or(0.0);
        let of = |want: &[SkillMatch]| -> Vec<String> { matches.iter().filter(|(_, m)| want.contains(m)).map(|(s, _)| s.clone()).collect() };
        let (full, partial, missing) = (of(&[SkillMatch::Listed]), of(&[SkillMatch::Related, SkillMatch::Mentioned]), of(&[SkillMatch::Missing]));
        let mut summary = Vec::new();
        for (label, list) in [("Has", &full), ("Partly", &partial), ("Missing", &missing)] {
            if !list.is_empty() {
                summary.push(format!("{label}: {}.", words(list)));
            }
        }
        log.push(if score > 0.0 {
            ev(at, "good", format!("Screened: {} skill match", pct(score)), summary.join(" "))
        } else {
            ev(at, "bad", "Screened: not qualified", format!("None of the required skills ({}) matched.", words(&r.job_skills)))
        });

        let audit: Vec<(String, String, String, Value, DateTime<Utc>)> = sqlx::query_as(
            "SELECT action, outcome, reasoning_summary, inputs, created_at FROM axom.audit_log
             WHERE action IN ('hiring.shortlist','hiring.invite','hiring.score','hiring.send_contract','hiring.no_show','hiring.complete','hiring.decline','hiring.close_others')
               AND (inputs->>'candidate_id' = $1 OR inputs->'shortlisted' ? $1 OR inputs->'candidates' ? $1)
             ORDER BY seq",
        )
        .bind(cid.to_string())
        .fetch_all(pool)
        .await?;
        for (action, outcome, reason, inputs, at) in audit {
            let e = match (action.as_str(), outcome.as_str()) {
                ("hiring.shortlist", _) => ev(at, "good", "Shortlisted for an AI interview", format!("Among the top {SHORTLIST_SIZE} by skill match.")),
                ("hiring.invite", "invited") => ev(at, "info", "Interview booked, invite emailed", "They got a join link to the AI interview."),
                ("hiring.invite", "send_failed") => ev(at, "warn", "Invite email failed", reason),
                ("hiring.invite", "rebooked") => ev(at, "warn", "Interview rebooked", reason),
                ("hiring.score", _) => {
                    let s = inputs["rubric_score"].as_f64().unwrap_or(0.0);
                    let tone = if s >= MIN_RUBRIC_SCORE { "good" } else { "bad" };
                    ev(at, tone, format!("Interview scored {s:.2}/5"), if s >= MIN_RUBRIC_SCORE { format!("At or above the {MIN_RUBRIC_SCORE} bar.") } else { format!("Below the {MIN_RUBRIC_SCORE} bar.") })
                }
                ("hiring.send_contract", "sent") => ev(at, "good", "Contract sent for signature", ""),
                ("hiring.send_contract", _) => ev(at, "bad", "Contract blocked by policy", reason),
                ("hiring.no_show", _) => ev(at, "bad", "Missed the interview", reason),
                ("hiring.complete", _) => ev(at, "good", "Contract signed: hired", ""),
                // The consent answer itself is logged from the meeting below.
                ("hiring.decline", _) if inputs["stage"] == "interview" => continue,
                ("hiring.decline", _) => ev(at, "bad", "Declined", reason),
                ("hiring.close_others", _) => ev(at, "bad", "Not selected", reason),
                _ => continue,
            };
            log.push(e);
        }
    }

    // The interview itself, from the meeting.
    if let Some(at) = r.first_joined_at.or_else(|| r.consent.as_ref().and_then(|c| c["at"].as_str()).and_then(|s| s.parse().ok())) {
        log.push(ev(at, "info", "Joined the interview", ""));
    }
    if let Some(c) = &r.consent {
        if let Some(at) = c["at"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()) {
            if c["granted"].as_bool() == Some(true) {
                log.push(ev(at, "good", "Agreed to be recorded", ""));
            } else {
                log.push(ev(at, "bad", "Declined to be recorded", "The interview ended at the consent question."));
            }
        }
    }
    if let (Some("completed"), Some(at)) = (r.meeting_status.as_deref(), r.meeting_completed_at) {
        log.push(ev(at, "info", "Interview finished", ""));
    }
    if let Some(at) = r.expired_at {
        log.push(ev(at, "warn", "Interview link expired", "Nobody joined in time."));
    }
    log.sort_by_key(|e| e.at);
    // Repeated audit + meeting entries for the same moment (consent decline) read once.
    log.dedup_by(|b, a| a.title == b.title);

    let assessment = assess(&r, now);
    let mut v = view(&r, now);
    v["log"] = json!(log);
    v["skill_match"] = json!(matches.iter().map(|(s, m)| json!({ "skill": s, "match": m })).collect::<Vec<_>>());
    v["interview"] = json!({
        "meeting_id": r.meeting_id, "status": r.meeting_status, "scheduled_at": r.scheduled_at,
        "link_expires_at": r.scheduled_at.map(meetings::link_expiry), "scores": r.scores,
        "pass_mark": MIN_RUBRIC_SCORE,
    });
    v["steps"] = json!(STEPS);
    v["progress"] = json!(assessment);
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn row() -> Row {
        Row {
            id: Uuid::nil(),
            job_id: Uuid::nil(),
            job_title: "Designer".into(),
            job_skills: vec!["graphic_design".into(), "branding".into()],
            job_status: "open".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            skills: vec!["graphic_design".into()],
            message: String::new(),
            created_at: Utc::now(),
            posting_status: Some("open".into()),
            candidate_id: None,
            status: None,
            shortlist_score: None,
            rubric_score: None,
            screened_at: None,
            updated_at: None,
            meeting_id: None,
            meeting_status: None,
            scheduled_at: None,
            first_joined_at: None,
            consent: None,
            scores: None,
            meeting_completed_at: None,
            expired_at: None,
            cv_filename: None,
            cv_size: None,
            in_pipeline: 0,
            other_offer: None,
            last_action: None,
            last_outcome: None,
            last_reason: None,
        }
    }

    fn with(status: &str, f: impl FnOnce(&mut Row)) -> Assessment {
        let mut r = row();
        r.candidate_id = Some(Uuid::nil());
        r.status = Some(status.into());
        f(&mut r);
        assess(&r, Utc::now())
    }

    #[test]
    fn new_application_waits_for_screening() {
        let x = assess(&row(), Utc::now());
        assert_eq!((x.stage, x.state, x.outcome), ("awaiting_screening", "waiting", "in_progress"));
    }

    #[test]
    fn no_skill_match_is_not_qualified_with_the_skills_named() {
        let x = with("applied", |r| {
            r.shortlist_score = Some(0.0);
            r.skills = vec!["accounting".into()];
        });
        assert_eq!((x.stage, x.outcome), ("not_qualified_skills", "not_qualified"));
        assert!(x.detail.contains("graphic design, branding") && x.detail.contains("accounting"), "{}", x.detail);
    }

    #[test]
    fn qualified_applicant_waits_for_a_slot_and_sees_what_is_missing() {
        let x = with("applied", |r| {
            r.shortlist_score = Some(0.5);
            r.in_pipeline = 3;
        });
        assert_eq!((x.stage, x.state), ("waiting_for_slot", "waiting"));
        assert!(x.headline.contains("50%") && x.detail.contains("Missing: branding"), "{} / {}", x.headline, x.detail);
    }

    #[test]
    fn failed_invite_is_shown_as_retrying() {
        let x = with("shortlisted", |r| {
            r.last_action = Some("hiring.invite".into());
            r.last_outcome = Some("send_failed".into());
        });
        assert_eq!(x.stage, "invite_failed");
    }

    #[test]
    fn invited_shows_the_link_expiry() {
        let x = with("invited", |r| {
            r.meeting_status = Some("dispatched".into());
            r.scheduled_at = Some(Utc::now() + Duration::minutes(5));
        });
        assert_eq!((x.stage, x.state), ("invited", "waiting"));
        assert!(x.detail.contains("left"), "{}", x.detail);
    }

    #[test]
    fn low_interview_score_is_not_qualified_with_the_weakest_criterion() {
        let x = with("interviewed", |r| {
            r.rubric_score = Some(2.75);
            r.scores = Some(json!({ "communication": 2, "skill_depth": 3 }));
        });
        assert_eq!((x.stage, x.outcome), ("below_bar", "not_qualified"));
        assert!(x.detail.contains("communication 2/5"), "{}", x.detail);
    }

    #[test]
    fn consent_decline_and_no_show_and_filled_have_reasons() {
        assert_eq!(with("declined", |r| r.meeting_status = Some("consent_declined".into())).stage, "declined_recording");
        assert_eq!(with("rejected", |r| r.last_action = Some("hiring.no_show".into())).stage, "no_show");
        let x = with("rejected", |r| {
            r.other_offer = Some("hired".into());
            r.rubric_score = Some(3.8);
        });
        assert_eq!((x.stage, x.outcome), ("position_filled", "not_selected"));
        assert!(x.detail.contains("filled by another candidate"), "{}", x.detail);
    }
}
