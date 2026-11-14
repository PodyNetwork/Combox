//! The opendesk CRM adapter against a fake opendesk that keeps its contacts:
//! contacts are found by exact email (opendesk's `q` search is fuzzy),
//! created once, and notes are appended, never replaced.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axom_core::decision::{Decision, Outcome};
use axom_tools::adapters::opendesk::OpendeskCrm;
use axom_tools::config::ConfigSource;
use axom_tools::*;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

const KEY: &str = "od_live_TEST-KEY-0123456789";

#[derive(Clone, Default)]
struct Desk {
    contacts: Arc<Mutex<Vec<Value>>>,
    posts: Arc<Mutex<usize>>,
}

fn authed(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {KEY}"))
}

async fn list(State(d): State<Desk>, h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    if !authed(&h) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "code": "unauthenticated" })));
    }
    // Like opendesk: a substring search over name and addresses.
    let needle = q.get("q").cloned().unwrap_or_default().to_lowercase();
    let hits: Vec<Value> = d.contacts.lock().unwrap().iter().filter(|c| c.to_string().to_lowercase().contains(&needle)).cloned().collect();
    (StatusCode::OK, Json(json!({ "data": hits, "next_cursor": null })))
}

async fn create(State(d): State<Desk>, h: HeaderMap, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    if !authed(&h) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    *d.posts.lock().unwrap() += 1;
    let mut contacts = d.contacts.lock().unwrap();
    let c = json!({ "id": format!("c{}", contacts.len() + 1), "email": b["email"], "emails": [], "name": b["name"], "notes": "", "custom": b.get("custom").cloned().unwrap_or(json!({})) });
    contacts.push(c.clone());
    (StatusCode::CREATED, Json(c))
}

async fn one(State(d): State<Desk>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    match d.contacts.lock().unwrap().iter().find(|c| c["id"] == id.as_str()) {
        Some(c) => (StatusCode::OK, Json(c.clone())),
        None => (StatusCode::NOT_FOUND, Json(json!({ "code": "not_found" }))),
    }
}

async fn patch(State(d): State<Desk>, Path(id): Path<String>, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut contacts = d.contacts.lock().unwrap();
    let Some(c) = contacts.iter_mut().find(|c| c["id"] == id.as_str()) else { return (StatusCode::NOT_FOUND, Json(json!({}))) };
    for (k, v) in b.as_object().unwrap() {
        c[k] = v.clone();
    }
    (StatusCode::OK, Json(c.clone()))
}

async fn desk() -> (Desk, String) {
    let d = Desk::default();
    let app = Router::new()
        .route("/v1/contacts", get(list).post(create))
        .route("/v1/contacts/{id}", get(one).patch(patch))
        .with_state(d.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (d, url)
}

fn auth() -> Authorized {
    let d = Decision { outcome: Outcome::Allow, reason: "ok".into(), policy_version: "test+1".into(), notify_owner: false };
    Authorized::from_decision("crm_write", &d).unwrap()
}

fn person(email: &str) -> Contact {
    Contact { name: email.into(), email: email.into(), company: None }
}

#[tokio::test]
async fn contacts_by_exact_email_and_notes_appended() {
    let (d, url) = desk().await;
    let crm = OpendeskCrm::new(&url, KEY, true);
    // Someone whose address merely contains the one we look for.
    d.contacts.lock().unwrap().push(json!({ "id": "decoy", "email": "ana@example.com.au", "emails": [], "name": "Other Ana", "notes": "" }));

    let id = crm.upsert_contact(&auth(), &person("Ana@Example.com")).await.unwrap();
    assert_ne!(id, "decoy", "a fuzzy match isn't the same person");
    assert_eq!(crm.upsert_contact(&auth(), &person("ana@example.com")).await.unwrap(), id, "found again, not created twice");
    assert_eq!(*d.posts.lock().unwrap(), 1);
    let created = d.contacts.lock().unwrap().iter().find(|c| c["id"] == id.as_str()).cloned().unwrap();
    assert_eq!(created["name"], "Ana", "an address-only contact gets the address's name");

    crm.add_note(&auth(), &id, "Meeting summary: Interview\n\nStrong portfolio.").await.unwrap();
    crm.add_note(&auth(), &id, "Second meeting.").await.unwrap();
    crm.upsert_deal(&auth(), "Brand refresh", &id, 1200.0).await.unwrap();
    let notes = d.contacts.lock().unwrap().iter().find(|c| c["id"] == id.as_str()).unwrap()["notes"].as_str().unwrap().to_string();
    assert!(notes.contains("Strong portfolio.") && notes.contains("Second meeting."), "{notes}");
    assert!(notes.contains("Deal: Brand refresh (USD 1200.00)"), "{notes}");
    assert!(notes.find("Strong portfolio.").unwrap() < notes.find("Second meeting.").unwrap(), "in order");
    assert_eq!(notes.matches("· ForeMan]").count(), 3, "each entry dated");

    // An extra address of an existing contact finds that contact.
    d.contacts.lock().unwrap().push(json!({ "id": "c9", "email": "bo@example.com", "emails": ["bo@work.example"], "name": "Bo", "notes": "" }));
    assert_eq!(crm.upsert_contact(&auth(), &person("bo@work.example")).await.unwrap(), "c9");
}

#[tokio::test]
async fn a_wrong_key_is_an_error_and_never_echoed() {
    let (_, url) = desk().await;
    let crm = OpendeskCrm::new(&url, "od_live_WRONG-KEY", true);
    let err = crm.upsert_contact(&auth(), &person("x@example.com")).await.unwrap_err();
    assert!(matches!(err, ToolError::NotAuthorized(_)), "{err:?}");
    assert!(!err.to_string().contains("WRONG-KEY"));
}

struct Map(HashMap<&'static str, &'static str>);
impl ConfigSource for Map {
    fn get(&self, k: &str) -> Option<String> {
        self.0.get(k).map(|v| v.to_string())
    }
}

#[test]
fn config_needs_a_key_and_a_real_url() {
    // The URL defaults to opendesk's hosted API.
    let ok = OpendeskCrm::from_config(&Map(HashMap::from([("CRM_API_KEY", KEY)]))).unwrap();
    assert!(format!("{:?}", ok.http).contains("https://api.opendesk.sh/v1"));
    // An email address in the URL field (an easy slip) is refused.
    let bad = OpendeskCrm::from_config(&Map(HashMap::from([("CRM_API_KEY", KEY), ("CRM_URL", "someone@icloud.com")])));
    assert!(bad.is_err());
    assert!(OpendeskCrm::from_config(&Map(HashMap::new())).is_err());
}
