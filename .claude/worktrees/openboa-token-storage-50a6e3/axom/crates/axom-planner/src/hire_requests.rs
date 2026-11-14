//! Hiring requests from AI workers.
//!
//! A worker given a task like "hire a copywriter" can't hire anyone, but it
//! can ask HR to: its `request_hire` tool opens a freelancer hiring request
//! for the task. The worker never posts, picks, pays, or signs anything. The
//! hiring pipeline takes the request from there under policy: a job posting
//! on the configured marketplace (ForeMan's jobs board or Upwork) with an
//! owner-approved contract template, a deterministic shortlist, the AI
//! interview, and the contract.
//!
//! Guardrails: human freelancers only (no new AI workers with tools), at
//! most one open request per task, and the title, skills, and description
//! are checked and trimmed before they are stored. A rate is optional (the
//! one the owner gave); it must be positive and no more than the payout
//! approval threshold, otherwise the planned role's rate is posted.

use axom_core::domain::{Actor, Task};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::routing::{self, FreelancerAsk};

/// The tool Claude calls to ask for a hire.
pub const REQUEST_HIRE_TOOL: &str = "request_hire";

const MAX_TITLE_CHARS: usize = 100;
/// Applicants are scored by the share of these they list, so keep it short.
const MAX_SKILLS: usize = 5;
const MAX_SKILL_CHARS: usize = 40;
const MAX_REASON_CHARS: usize = 500;
const MAX_DESCRIPTION_CHARS: usize = 3000;

/// The `request_hire` tool as offered to Claude.
pub fn request_hire_tool() -> Value {
    json!({
        "name": REQUEST_HIRE_TOOL,
        "description": "Ask HR to hire a human freelancer for this task. It opens a hiring request; ForeMan then posts the \
            job on its jobs board (or Upwork, if that's what the company uses) after a policy check, shortlists \
            applicants, runs an AI interview, and sends an owner-approved contract, with no further approval needed; \
            the owner is told when the contract goes out. You don't post, choose, or pay anyone yourself. Use it when \
            your task is to hire someone, or when the work needs a person no one at the company covers. One request \
            per task.",
        "input_schema": {
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "The job title to post, e.g. \"Freelance Copywriter\"." },
                "skills": {
                    "type": "array", "items": { "type": "string" },
                    "description": "The 1 to 5 core skills the hire needs, as short common snake_case words, e.g. [\"copywriting\", \"seo\"]. Applicants are shortlisted by the share of these they list, so name only what matters most.",
                },
                "description": { "type": "string", "description": "The job post applicants read: the work, what a good applicant brings, and what to include when applying. Plain text, up to 3000 characters." },
                "rate_usd": { "type": "number", "description": "Pay per task in USD, only if the owner gave one. Leave it out otherwise and the company's usual rate is posted." },
                "reason": { "type": "string", "description": "One or two sentences on why this hire is needed, for the people who review hiring." },
            },
            "required": ["title", "skills", "reason"],
        },
    })
}

/// A checked request, or why it was refused (sent back to Claude).
#[derive(Debug, PartialEq)]
pub struct Request {
    pub title: String,
    pub skills: Vec<String>,
    pub description: String,
    pub rate_usd: Option<Decimal>,
    pub reason: String,
}

/// Single spaces, no control characters, at most `max` characters.
fn clean(s: &str, max: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// Like `clean`, but keeps line breaks (at most one blank line in a row).
fn clean_text(s: &str, max: usize) -> String {
    let lines: Vec<String> = s.lines().map(|l| clean(l, max)).collect();
    let mut out: Vec<&str> = Vec::new();
    for l in &lines {
        if l.is_empty() && out.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        out.push(l);
    }
    out.join("\n").chars().take(max).collect::<String>().trim().to_string()
}

/// `"SEO Writing"` → `"seo_writing"`.
fn skill(s: &str) -> String {
    let lowered: String = s.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    lowered.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_").chars().take(MAX_SKILL_CHARS).collect()
}

/// Check a request. `max_rate` is the highest per-task rate accepted.
pub fn parse(args: &Value, max_rate: Decimal) -> Result<Request, String> {
    let title = clean(args["title"].as_str().unwrap_or(""), MAX_TITLE_CHARS);
    if title.chars().count() < 3 {
        return Err("give the job a title, e.g. \"Freelance Copywriter\"".into());
    }
    let mut skills: Vec<String> = Vec::new();
    for s in args["skills"].as_array().into_iter().flatten().filter_map(Value::as_str).map(skill) {
        if !s.is_empty() && !skills.contains(&s) {
            skills.push(s);
        }
    }
    if skills.is_empty() {
        return Err("list at least one skill the hire needs, e.g. [\"copywriting\"]".into());
    }
    if skills.len() > MAX_SKILLS {
        return Err(format!("name at most {MAX_SKILLS} core skills; applicants are scored by the share of them they list"));
    }
    let rate_usd = match &args["rate_usd"] {
        Value::Null => None,
        v => {
            let r = v.as_f64().and_then(Decimal::from_f64).map(|r| r.round_dp(2)).filter(|r| *r > Decimal::ZERO);
            match r {
                None => return Err("rate_usd must be a positive number of US dollars per task".into()),
                Some(r) if r > max_rate => {
                    return Err(format!(
                        "a rate above ${max_rate} per task can't be posted from a hiring request; leave rate_usd out (the usual rate is posted) and tell the owner"
                    ))
                }
                Some(r) => Some(r),
            }
        }
    };
    let description = clean_text(args["description"].as_str().unwrap_or(""), MAX_DESCRIPTION_CHARS);
    let reason = clean(args["reason"].as_str().unwrap_or(""), MAX_REASON_CHARS);
    let reason = if reason.is_empty() { "requested by an AI worker".to_string() } else { reason };
    Ok(Request { title, skills, description, rate_usd, reason })
}

/// Handle one `request_hire` call. Returns the text for Claude and whether
/// it is an error.
pub async fn handle(pool: &PgPool, actor: &Actor, task: &Task, args: &Value, max_rate: Decimal) -> anyhow::Result<(String, bool)> {
    let req = match parse(args, max_rate) {
        Ok(r) => r,
        Err(e) => return Ok((format!("Not opened: {e}."), true)),
    };
    let reason = format!("{} (asked for by the AI worker on task: {})", req.reason, clean(&task.objective, 200));
    let ask = FreelancerAsk { title: req.title, skills: req.skills, description: req.description, rate_usd: req.rate_usd, reason };
    Ok(match routing::open_freelancer_request(pool, actor, task.id, &ask).await? {
        Some(id) => (
            format!(
                "Hiring request {id} opened for \"{}\" (skills: {}; rate: {}; description: {}). ForeMan posts the job \
                 on its next hiring pass after the policy check, with no further approval, then shortlists, \
                 interviews, and sends the contract; the owner is told when the contract goes out. Nothing more is \
                 needed from you for the hire.",
                ask.title,
                ask.skills.join(", "),
                ask.rate_usd.map(|r| format!("${r:.2} per task")).unwrap_or_else(|| "the company's usual rate".into()),
                if ask.description.is_empty() { "none, the post shows only title, skills, and rate".to_string() } else { format!("{} characters", ask.description.chars().count()) },
            ),
            false,
        ),
        None => ("This task already has an open hiring request; it is being handled. Don't open another.".into(), true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max() -> Decimal {
        Decimal::from(50)
    }

    #[test]
    fn parses_and_tidies_a_request() {
        let r = parse(
            &json!({ "title": "  Freelance\n Copywriter ", "skills": ["Copywriting", "SEO writing", "copywriting", ""], "reason": "Owner asked.",
                     "description": "Write web copy.\n\n\n\nSend a  portfolio.\u{0007}", "rate_usd": 25 }),
            max(),
        )
        .unwrap();
        assert_eq!(
            r,
            Request {
                title: "Freelance Copywriter".into(),
                skills: vec!["copywriting".into(), "seo_writing".into()],
                description: "Write web copy.\n\nSend a portfolio.".into(),
                rate_usd: Some(Decimal::from(25)),
                reason: "Owner asked.".into(),
            }
        );
    }

    #[test]
    fn refuses_a_request_without_title_or_skills() {
        assert!(parse(&json!({ "title": "", "skills": ["copywriting"] }), max()).is_err());
        assert!(parse(&json!({ "title": "Copywriter", "skills": ["!!"] }), max()).is_err());
        assert!(parse(&json!({ "title": "Copywriter" }), max()).is_err());
    }

    #[test]
    fn refuses_too_many_skills_and_bad_rates() {
        let many: Vec<String> = (0..6).map(|i| format!("skill{i}")).collect();
        assert!(parse(&json!({ "title": "Copywriter", "skills": many }), max()).unwrap_err().contains("at most 5"));
        let ok = json!({ "title": "Copywriter", "skills": ["copywriting"] });
        for bad in [json!(0), json!(-5), json!("25"), json!(50.01)] {
            let mut a = ok.clone();
            a["rate_usd"] = bad.clone();
            assert!(parse(&a, max()).is_err(), "{bad}");
        }
        let mut a = ok.clone();
        a["rate_usd"] = json!(50);
        assert_eq!(parse(&a, max()).unwrap().rate_usd, Some(Decimal::from(50)));
        assert_eq!(parse(&ok, max()).unwrap().rate_usd, None);
    }

    #[test]
    fn caps_lengths() {
        let r = parse(&json!({ "title": "x".repeat(500), "skills": ["a"], "reason": "", "description": "y".repeat(5000) }), max()).unwrap();
        assert_eq!(r.title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(r.description.chars().count(), MAX_DESCRIPTION_CHARS);
        assert_eq!(r.reason, "requested by an AI worker");
    }
}
