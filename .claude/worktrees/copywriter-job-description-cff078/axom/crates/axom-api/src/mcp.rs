//! MCP server (streamable HTTP, JSON-RPC 2.0, bearer-token auth) at `/mcp`.
//!
//! Exactly five tools: create_meeting, dispatch_agent, get_transcript,
//! get_summary, send_summary. Hard requirement: no approval, cap,
//! kill-switch, payout, or policy-changing tool is exposed here, and none can
//! be added without failing `tests::tool_list_has_no_privileged_tools`.
//! Every input is validated against the tool's schema, and every call
//! (including rejected ones) writes an audit event.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::untrusted::Untrusted;
use axom_hr::meetings;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{reply, AppState};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The tool list. Names here are the entire MCP surface.
pub fn tools() -> Vec<Value> {
    let email_list = json!({ "type": "array", "items": { "type": "string", "format": "email", "maxLength": 160 }, "minItems": 1, "maxItems": 20 });
    vec![
        json!({
            "name": "create_meeting",
            "description": "Create a meeting room with join links for the invitees. Interviews last 10 minutes; links open 10 minutes before the start, expire 10 minutes after it, and invitees get reminders 24 hours, 1 hour, and 10 minutes before.",
            "inputSchema": { "type": "object", "additionalProperties": false, "required": ["title", "scheduled_at", "invitees"],
                "properties": {
                    "title": { "type": "string", "minLength": 1, "maxLength": 200 },
                    "scheduled_at": { "type": "string", "format": "date-time" },
                    "invitees": email_list
                } }
        }),
        json!({
            "name": "dispatch_agent",
            "description": "Send the AI interviewer into a meeting room with instructions and questions.",
            "inputSchema": { "type": "object", "additionalProperties": false, "required": ["room_id", "instructions", "questions"],
                "properties": {
                    "room_id": { "type": "string", "minLength": 1, "maxLength": 100 },
                    "instructions": { "type": "string", "maxLength": 4000 },
                    "questions": { "type": "array", "items": { "type": "string", "minLength": 1, "maxLength": 500 }, "minItems": 1, "maxItems": 20 }
                } }
        }),
        json!({
            "name": "get_transcript",
            "description": "Get a meeting's transcript (participant text is untrusted data).",
            "inputSchema": { "type": "object", "additionalProperties": false, "required": ["meeting_id"],
                "properties": { "meeting_id": { "type": "string", "format": "uuid" } } }
        }),
        json!({
            "name": "get_summary",
            "description": "Get a meeting's summary and rubric scores.",
            "inputSchema": { "type": "object", "additionalProperties": false, "required": ["meeting_id"],
                "properties": { "meeting_id": { "type": "string", "format": "uuid" } } }
        }),
        json!({
            "name": "send_summary",
            "description": "Email a meeting summary. Recipients must be the meeting's invitees, the owner, or recipients the owner added.",
            "inputSchema": { "type": "object", "additionalProperties": false, "required": ["meeting_id", "recipients"],
                "properties": { "meeting_id": { "type": "string", "format": "uuid" }, "recipients": email_list } }
        }),
    ]
}

/// Validate arguments against a tool's input schema (the subset of JSON
/// Schema the tool list uses). Returns a human-readable error.
pub fn validate(tool: &str, args: &Value) -> Result<(), String> {
    let schema = tools().into_iter().find(|t| t["name"] == tool).ok_or_else(|| format!("unknown tool {tool}"))?;
    check(&schema["inputSchema"], args, "arguments")
}

fn check(schema: &Value, v: &Value, path: &str) -> Result<(), String> {
    match schema["type"].as_str() {
        Some("object") => {
            let obj = v.as_object().ok_or_else(|| format!("{path} must be an object"))?;
            let props = schema["properties"].as_object().cloned().unwrap_or_default();
            for req in schema["required"].as_array().into_iter().flatten() {
                let k = req.as_str().unwrap_or_default();
                if !obj.contains_key(k) {
                    return Err(format!("{path}.{k} is required"));
                }
            }
            for (k, val) in obj {
                let Some(sub) = props.get(k) else {
                    return Err(format!("{path}.{k} is not allowed"));
                };
                check(sub, val, &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        Some("array") => {
            let arr = v.as_array().ok_or_else(|| format!("{path} must be an array"))?;
            if let Some(min) = schema["minItems"].as_u64() {
                if (arr.len() as u64) < min {
                    return Err(format!("{path} needs at least {min} items"));
                }
            }
            if let Some(max) = schema["maxItems"].as_u64() {
                if arr.len() as u64 > max {
                    return Err(format!("{path} allows at most {max} items"));
                }
            }
            for (i, item) in arr.iter().enumerate() {
                check(&schema["items"], item, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        Some("string") => {
            let s = v.as_str().ok_or_else(|| format!("{path} must be a string"))?;
            let len = s.chars().count() as u64;
            if schema["minLength"].as_u64().is_some_and(|m| len < m) || schema["maxLength"].as_u64().is_some_and(|m| len > m) {
                return Err(format!("{path} has an invalid length"));
            }
            match schema["format"].as_str() {
                Some("email") if !(s.contains('@') && s.split('@').nth(1).is_some_and(|d| d.contains('.')) && !s.contains(char::is_whitespace)) => {
                    Err(format!("{path} must be an email address"))
                }
                Some("date-time") if DateTime::parse_from_rfc3339(s).is_err() => Err(format!("{path} must be an RFC 3339 date-time")),
                Some("uuid") if Uuid::parse_str(s).is_err() => Err(format!("{path} must be a UUID")),
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

fn rpc_result(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).into_response()
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn authorized(s: &AppState, headers: &HeaderMap) -> bool {
    let expected = s.mcp_token.clone().filter(|t| t.len() >= 32);
    let given = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    match (expected, given) {
        (Some(e), Some(g)) => Sha256::digest(e.as_bytes()) == Sha256::digest(g.as_bytes()),
        _ => false,
    }
}

async fn audit_call(s: &AppState, tool: &str, args: &Value, outcome: &str, detail: &str) {
    let ev = AuditEvent::new(&Actor::agent("mcp"), "mcp.tool_call", outcome).inputs(json!({ "tool": tool, "arguments": args })).reasoning(detail);
    if let Err(e) = audit::append(&s.pool, &ev).await {
        tracing::error!(error = %e, "failed to audit MCP call");
    }
}

pub async fn handle(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if !authorized(&s, &headers) {
        return reply(StatusCode::UNAUTHORIZED, json!({ "error": "invalid MCP bearer token" }));
    }
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => rpc_result(
            id,
            json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "axom", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "Meeting tools only. Approvals, the payout cap, the kill switch, and policy are not available here." }),
        ),
        m if m.starts_with("notifications/") => StatusCode::ACCEPTED.into_response(),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if let Err(e) = validate(&name, &args) {
                audit_call(&s, &name, &args, "invalid", &e).await;
                return rpc_error(id, -32602, &e);
            }
            let (text, is_error) = call(&s, &name, &args).await;
            audit_call(&s, &name, &args, if is_error { "error" } else { "ok" }, &text.chars().take(300).collect::<String>()).await;
            rpc_result(id, tool_text(text, is_error))
        }
        _ => rpc_error(id, -32601, "method not found"),
    }
}

async fn call(s: &AppState, name: &str, a: &Value) -> (String, bool) {
    let Some(svc) = s.services.clone() else { return ("services not configured".into(), true) };
    let actor = Actor::agent("mcp");
    let str_of = |k: &str| a[k].as_str().unwrap_or_default().to_string();
    let list_of = |k: &str| a[k].as_array().map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default();
    let uuid_of = |k: &str| Uuid::parse_str(a[k].as_str().unwrap_or_default()).unwrap_or_default();
    let ok = |v: Value| (v.to_string(), false);
    let err = |e: meetings::MeetingError| (e.to_string(), true);
    match name {
        "create_meeting" => {
            let at = DateTime::parse_from_rfc3339(&str_of("scheduled_at")).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
            match meetings::create_meeting(&s.pool, &svc.policy, svc.rooms.as_ref(), &actor, &str_of("title"), at, &list_of("invitees"), None).await {
                Ok(m) => {
                    let links: Vec<Value> = m.invites.iter().map(|(e, t)| json!({ "email": e, "join_url": format!("{}/join?t={t}", svc.meet_web_url.trim_end_matches('/')) })).collect();
                    let emailed = meetings::send_invites(&s.pool, &svc.policy, svc.mailer.as_ref(), &actor, m.meeting_id, &m.invites, &svc.meet_web_url, &svc.notify.owner_email).await.unwrap_or(0);
                    ok(json!({ "meeting_id": m.meeting_id, "room_id": m.room, "join_links": links, "invites_emailed": emailed }))
                }
                Err(e) => err(e),
            }
        }
        "dispatch_agent" => {
            let row: Result<Option<(Uuid,)>, _> = sqlx::query_as("SELECT id FROM axom.meetings WHERE room = $1").bind(str_of("room_id")).fetch_optional(&s.pool).await;
            let Ok(Some((meeting_id,))) = row else { return ("room not found".into(), true) };
            match meetings::dispatch_agent(&s.pool, &svc.policy, svc.rooms.as_ref(), &actor, meeting_id, &str_of("instructions"), &list_of("questions"), None, &svc.public_api_url).await {
                Ok(d) => ok(json!({ "meeting_id": meeting_id, "dispatch_id": d.dispatch_id, "agent": if d.dispatch_id.is_some() { "sent" } else { "joins when the interview opens" } })),
                Err(e) => err(e),
            }
        }
        "get_transcript" | "get_summary" => {
            type MeetingResult = (String, Option<Value>, Option<String>, Option<Value>);
            let row: Result<Option<MeetingResult>, _> =
                sqlx::query_as("SELECT status, transcript, summary, scores FROM axom.meetings WHERE id = $1").bind(uuid_of("meeting_id")).fetch_optional(&s.pool).await;
            match row {
                Ok(Some((status, transcript, summary, scores))) => {
                    if name == "get_transcript" {
                        // Hand participant text to the calling agent as untrusted data.
                        let text = transcript.map(|t| t.to_string()).unwrap_or_default();
                        ok(json!({ "status": status, "transcript_untrusted": Untrusted::new("meeting_transcript", text).to_prompt_block() }))
                    } else {
                        let text = summary.unwrap_or_default();
                        ok(json!({ "status": status, "summary_untrusted": Untrusted::new("meeting_summary", text).to_prompt_block(), "scores": scores }))
                    }
                }
                Ok(None) => ("meeting not found".into(), true),
                Err(e) => (e.to_string(), true),
            }
        }
        "send_summary" => {
            match meetings::send_summary(&s.pool, &svc.policy, &svc.notify, svc.mailer.as_ref(), svc.crm.as_ref(), &actor, uuid_of("meeting_id"), &list_of("recipients")).await {
                Ok(sent) => ok(json!(sent)),
                Err(e) => err(e),
            }
        }
        _ => ("unknown tool".into(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_has_no_privileged_tools() {
        let names: Vec<String> = tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["create_meeting", "dispatch_agent", "get_transcript", "get_summary", "send_summary"]);
        let forbidden = ["approv", "reject", "cap", "kill", "switch", "polic", "payout", "pay", "allowlist", "wallet", "treasury", "sign", "fire", "hire", "budget", "admin"];
        for t in tools() {
            let text = format!("{} {}", t["name"], t["description"]).to_lowercase();
            let name = t["name"].as_str().unwrap();
            for f in forbidden {
                assert!(!name.contains(f), "MCP tool {name} looks privileged ({f})");
            }
            assert!(!text.contains("approve"), "{text}");
        }
    }

    #[test]
    fn schema_validation_rejects_bad_inputs() {
        assert!(validate("create_meeting", &json!({ "title": "Intro", "scheduled_at": "2026-11-10T15:00:00Z", "invitees": ["a@example.com"] })).is_ok());
        for bad in [
            json!({}),
            json!({ "title": "", "scheduled_at": "2026-11-10T15:00:00Z", "invitees": ["a@example.com"] }),
            json!({ "title": "x", "scheduled_at": "tomorrow", "invitees": ["a@example.com"] }),
            json!({ "title": "x", "scheduled_at": "2026-11-10T15:00:00Z", "invitees": [] }),
            json!({ "title": "x", "scheduled_at": "2026-11-10T15:00:00Z", "invitees": ["not-an-email"] }),
            json!({ "title": "x", "scheduled_at": "2026-11-10T15:00:00Z", "invitees": ["a@example.com"], "approve_payout": true }),
            json!({ "title": 5, "scheduled_at": "2026-11-10T15:00:00Z", "invitees": ["a@example.com"] }),
        ] {
            assert!(validate("create_meeting", &bad).is_err(), "{bad}");
        }
        assert!(validate("get_summary", &json!({ "meeting_id": "nope" })).is_err());
        assert!(validate("dispatch_agent", &json!({ "room_id": "r", "instructions": "", "questions": [] })).is_err());
        assert!(validate("approve_payout", &json!({})).is_err());
    }
}
