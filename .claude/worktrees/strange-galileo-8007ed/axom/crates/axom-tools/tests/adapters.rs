//! Adapter tests against a local fake vendor server: dry-run makes no calls,
//! adapters refuse without a matching policy decision, vendor errors map to
//! typed errors, and credentials never appear in errors or debug output.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axom_core::decision::{Decision, Outcome};
use axom_tools::adapters::docuseal::DocuSeal;
use axom_tools::adapters::postiz::Postiz;
use axom_tools::adapters::smtp::{ics_invite, SmtpMailer};
use axom_tools::adapters::opendesk::OpendeskCrm;
use axom_tools::adapters::warmbly::Warmbly;
use axom_tools::mock::MockTools;
use axom_tools::*;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

const KEY: &str = "sk-live-SUPER-SECRET-0123456789";

type Hit = (String, String, Option<String>);

#[derive(Clone, Default)]
struct Vendor {
    hits: Arc<Mutex<Vec<Hit>>>,
    mode: Arc<Mutex<String>>,
}

async fn handler(State(v): State<Vendor>, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let auth = headers.get("authorization").or(headers.get("x-auth-token")).and_then(|h| h.to_str().ok()).map(str::to_string);
    v.hits.lock().unwrap().push((method.to_string(), uri.to_string(), auth.clone()));
    let mode = v.mode.lock().unwrap().clone();
    match mode.as_str() {
        "500" => (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response(),
        "redirect" => (StatusCode::FOUND, [("location", "/somewhere-else")]).into_response(),
        "429" => (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "17")], "slow down").into_response(),
        "422" => (StatusCode::UNPROCESSABLE_ENTITY, "{\"error\":\"bad field\"}").into_response(),
        // A careless vendor that echoes the credential back.
        "401echo" => (StatusCode::UNAUTHORIZED, format!("invalid key {}", auth.unwrap_or_default())).into_response(),
        _ => {
            let path = uri.path();
            let body: Value = if path.ends_with("/submissions") && method == Method::POST {
                json!([{ "id": 11, "submission_id": 42 }])
            } else if path.contains("/submissions/") {
                json!({ "status": "completed", "submitters": [{ "id": 11, "opened_at": "2026-11-08T00:00:00Z" }] })
            } else if path.ends_with("/submissions") {
                json!({ "data": [{ "status": "completed" }] })
            } else if path == "/v1/contacts" && method == Method::GET {
                json!({ "data": [], "next_cursor": null })
            } else if path == "/v1/contacts" {
                json!({ "id": "rec-1", "email": "ada@example.com", "notes": "" })
            } else if path.ends_with("/posts") {
                json!([{ "postId": "post-9" }])
            } else if path.ends_with("/stats") {
                json!({ "sent": 10, "replies": 3, "positive": 1 })
            } else if path.ends_with("/leads") {
                json!({ "added": 2 })
            } else {
                json!({ "id": "camp-1" })
            };
            axum::Json(body).into_response()
        }
    }
}

async fn vendor() -> (Vendor, String) {
    let v = Vendor::default();
    *v.mode.lock().unwrap() = "ok".into();
    let app = axum::Router::new().fallback(handler).with_state(v.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (v, url)
}

fn auth(action: &str) -> Authorized {
    let d = Decision { outcome: Outcome::Allow, reason: "ok".into(), policy_version: "test+1".into(), notify_owner: false };
    Authorized::from_decision(action, &d).unwrap()
}

fn templates() -> HashMap<String, i64> {
    HashMap::from([("freelancer-services-v1".into(), 1), ("tax-form-w9".into(), 2)])
}

fn contact() -> Contact {
    Contact { name: "Ada Lovelace".into(), email: "ada@example.com".into(), company: None }
}

#[tokio::test]
async fn dry_run_makes_no_external_calls() {
    let (v, url) = vendor().await;
    let ds = DocuSeal::new(&url, KEY, templates(), false);
    let wb = Warmbly::new(&url, KEY, false);
    let pz = Postiz::new(&url, KEY, false);
    let crm = OpendeskCrm::new(&url, KEY, false);
    ds.create_from_template(&auth("send_contract"), "freelancer-services-v1", &[Signer { name: "A".into(), email: "a@x.io".into() }], json!({}))
        .await
        .unwrap();
    ds.send(&auth("send_contract"), "1").await.unwrap();
    wb.create_campaign(&auth("send_external_message"), "c", "hi").await.unwrap();
    wb.add_contacts(&auth("send_external_message"), "c", &[contact()]).await.unwrap();
    pz.schedule_post(&auth("send_external_message"), &["x".into()], "hello", chrono::Utc::now()).await.unwrap();
    crm.upsert_contact(&auth("execute_tool"), &contact()).await.unwrap();
    crm.add_note(&auth("execute_tool"), "p1", "note").await.unwrap();
    assert!(v.hits.lock().unwrap().is_empty(), "no request reached the vendor");
    assert!(ds.http.requests().iter().all(|r| r.dry_run));
    assert!(!pz.http.requests().is_empty(), "dry-run requests are recorded");

    let mail = SmtpMailer::dry_run("axom@example.com");
    let ics = ics_invite("m1", "Interview", chrono::Utc::now(), 30, "axom@example.com", "https://meet.example.com/r/1");
    let e = Email { to: vec!["cand@example.com".into()], subject: "Interview".into(), body: "Join".into(), ics: Some(ics.clone()) };
    assert_eq!(mail.send(&auth("send_external_message"), &e).await.unwrap(), "dry-run");
    let built = String::from_utf8(mail.build(&e).unwrap().formatted()).unwrap();
    assert!(built.contains("text/calendar"), "ICS attached");
    assert!(ics.contains("DTSTART:") && ics.contains("SUMMARY:Interview"));
}

#[tokio::test]
async fn live_mode_calls_vendor_and_parses_responses() {
    let (v, url) = vendor().await;
    let ds = DocuSeal::new(&url, KEY, templates(), true);
    assert_eq!(ds.create_from_template(&auth("send_contract"), "freelancer-services-v1", &[], json!({})).await.unwrap(), "42");
    assert_eq!(ds.status("42").await.unwrap(), SubmissionStatus::Completed);
    assert_eq!(ds.fetch_tax_form("ada@example.com").await.unwrap(), Some(TaxFormKind::W9));
    let crm = OpendeskCrm::new(&url, KEY, true);
    assert_eq!(crm.upsert_contact(&auth("execute_tool"), &contact()).await.unwrap(), "rec-1");
    let wb = Warmbly::new(&url, KEY, true);
    assert_eq!(wb.reply_stats("c").await.unwrap(), ReplyStats { sent: 10, replies: 3, positive: 1 });
    let pz = Postiz::new(&url, KEY, true);
    assert_eq!(pz.schedule_post(&auth("send_external_message"), &["ch".into()], "x", chrono::Utc::now()).await.unwrap(), "post-9");
    let hits = v.hits.lock().unwrap();
    assert!(hits.iter().any(|(m, p, _)| m == "POST" && p == "/api/submissions"));
    assert!(hits.iter().any(|(_, p, a)| p.starts_with("/v1/contacts?q=") && a.as_deref() == Some(format!("Bearer {KEY}").as_str())));
    assert!(hits.iter().any(|(m, p, _)| m == "POST" && p == "/v1/contacts"));
    assert!(hits.iter().any(|(_, p, a)| p == "/public/v1/posts" && a.as_deref() == Some(KEY)));
}

#[tokio::test]
async fn adapters_refuse_without_a_matching_policy_decision() {
    let (v, url) = vendor().await;
    // No allow decision means no Authorized value can exist.
    let deny = Decision { outcome: Outcome::Deny, reason: "no".into(), policy_version: "v".into(), notify_owner: false };
    assert!(Authorized::from_decision("send_contract", &deny).is_none());
    let approval = Decision { outcome: Outcome::RequireApproval, ..deny.clone() };
    assert!(Authorized::from_decision("send_contract", &approval).is_none());

    // A decision for a different action is refused.
    let ds = DocuSeal::new(&url, KEY, templates(), true);
    assert!(matches!(ds.create_from_template(&auth("execute_tool"), "freelancer-services-v1", &[], json!({})).await, Err(ToolError::NotAuthorized(_))));
    assert!(matches!(Warmbly::new(&url, KEY, true).create_campaign(&auth("fire"), "c", "b").await, Err(ToolError::NotAuthorized(_))));
    assert!(matches!(Postiz::new(&url, KEY, true).schedule_post(&auth("hire"), &[], "x", chrono::Utc::now()).await, Err(ToolError::NotAuthorized(_))));
    assert!(matches!(OpendeskCrm::new(&url, KEY, true).add_note(&auth("payout"), "p", "n").await, Err(ToolError::NotAuthorized(_))));
    let mail = SmtpMailer::dry_run("axom@example.com");
    let e = Email { to: vec!["a@example.com".into()], subject: "s".into(), body: "b".into(), ics: None };
    assert!(matches!(mail.send(&auth("payout"), &e).await, Err(ToolError::NotAuthorized(_))));
    // Templates outside the owner-approved list are refused.
    assert!(matches!(ds.create_from_template(&auth("send_contract"), "custom-contract", &[], json!({})).await, Err(ToolError::NotAuthorized(_))));
    assert!(v.hits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn vendor_errors_map_to_typed_errors() {
    let (v, url) = vendor().await;
    let pz = Postiz::new(&url, KEY, true);
    let a = auth("send_external_message");
    *v.mode.lock().unwrap() = "500".into();
    assert!(matches!(pz.schedule_post(&a, &["c".into()], "x", chrono::Utc::now()).await, Err(ToolError::Retryable(_))));
    *v.mode.lock().unwrap() = "429".into();
    assert_eq!(pz.schedule_post(&a, &["c".into()], "x", chrono::Utc::now()).await, Err(ToolError::RateLimited { retry_after_secs: 17 }));
    *v.mode.lock().unwrap() = "422".into();
    assert!(matches!(pz.schedule_post(&a, &["c".into()], "x", chrono::Utc::now()).await, Err(ToolError::Permanent(_))));
    // A network failure is retryable.
    let down = Postiz::new("http://127.0.0.1:1", KEY, true);
    assert!(matches!(down.schedule_post(&a, &[], "x", chrono::Utc::now()).await, Err(ToolError::Retryable(_))));
}

#[tokio::test]
async fn credentials_never_appear_in_errors_or_debug_output() {
    let (v, url) = vendor().await;
    *v.mode.lock().unwrap() = "401echo".into();
    let debug_outputs = vec![
        format!("{:?}", DocuSeal::new(&url, KEY, templates(), true).http),
        format!("{:?}", Warmbly::new(&url, KEY, true).http),
        format!("{:?}", SmtpMailer::live("smtp.example.com", 587, "user", KEY, "a@example.com").unwrap()),
    ];
    for d in debug_outputs {
        assert!(!d.contains(KEY), "debug output leaks the key: {d}");
    }
    let err = OpendeskCrm::new(&url, KEY, true).upsert_contact(&auth("execute_tool"), &contact()).await.unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, ToolError::NotAuthorized(_)));
    assert!(text.contains("[redacted]"), "{text}");
    assert!(!text.contains(KEY), "{text}");
    let err = DocuSeal::new(&url, KEY, templates(), true).status("1").await.unwrap_err().to_string();
    assert!(!err.contains(KEY));
}

#[tokio::test]
async fn every_mock_implements_its_trait_fully() {
    let m = MockTools::new().with_tax_form("a@x.io", TaxFormKind::W8Ben);
    let a = auth("any");
    let sub = ContractService::create_from_template(&m, &a, "t", &[], json!({})).await.unwrap();
    ContractService::send(&m, &a, &sub).await.unwrap();
    ContractService::status(&m, &sub).await.unwrap();
    assert_eq!(m.fetch_tax_form("a@x.io").await.unwrap(), Some(TaxFormKind::W8Ben));
    let c = m.create_campaign(&a, "n", "b").await.unwrap();
    m.update_campaign(&a, &c, "b2").await.unwrap();
    m.add_contacts(&a, &c, &[contact()]).await.unwrap();
    m.reply_stats(&c).await.unwrap();
    m.schedule_post(&a, &["x".into()], "c", chrono::Utc::now()).await.unwrap();
    let p = m.upsert_contact(&a, &contact()).await.unwrap();
    m.upsert_deal(&a, "d", &p, 10.0).await.unwrap();
    m.add_note(&a, &p, "n").await.unwrap();
    let r = m.create_room(&a, "r").await.unwrap();
    m.participant_token(&a, &r.room, "u").await.unwrap();
    m.end_room(&a, &r.room).await.unwrap();
    Mailer::send(&m, &a, &Email { to: vec!["o@x.io".into()], subject: "s".into(), body: "b".into(), ics: None }).await.unwrap();
    m.submit(&a, &IntakeTask { task_id: uuid::Uuid::new_v4(), brief: "b".into(), required_skills: vec![], assignee: None }).await.unwrap();
    let j = m.post_job(&a, &JobSpec { title: "t".into(), skills: vec![], rate_usd: 1.0, kpis: vec![], description: String::new() }).await.unwrap();
    m.list_applicants(&j).await.unwrap();
    m.message_candidate(&a, "x", "hi").await.unwrap();
    assert_eq!(m.calls().len(), 20);
}

/// A service address typed in by a company on the platform is only contacted
/// through the strict guard: nothing is sent to a private address, and no
/// adapter follows a redirect (which would carry its credentials along).
#[tokio::test]
async fn company_supplied_addresses_are_guarded_and_redirects_are_not_followed() {
    use axom_tools::adapters::mcp::UrlGuard;
    let (v, url) = vendor().await;
    let mut crm = OpendeskCrm::new(&url, KEY, true);
    crm.http = crm.http.guarded(UrlGuard::strict());
    let err = crm.upsert_contact(&auth("crm_write"), &contact()).await.unwrap_err();
    assert!(matches!(err, ToolError::Permanent(_)), "{err:?}");
    assert!(!format!("{err:?}").contains(KEY));
    assert!(v.hits.lock().unwrap().is_empty(), "nothing was sent to the loopback address");

    // Unguarded (the operator's own .env): reachable, but a redirect is an
    // error, not a second request.
    let crm = OpendeskCrm::new(&url, KEY, true);
    *v.mode.lock().unwrap() = "redirect".into();
    assert!(crm.upsert_contact(&auth("crm_write"), &contact()).await.is_err());
    let hits = v.hits.lock().unwrap().clone();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(!hits[0].1.contains("somewhere-else"));
}
