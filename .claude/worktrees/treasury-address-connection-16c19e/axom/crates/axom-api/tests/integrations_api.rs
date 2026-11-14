//! Requires DATABASE_URL. Integration settings set from the dashboard
//! (single-company mode) and the interview agent's key lookup.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::{router, AppState, ApprovalConfig, Services};
use axom_core::authz::AuthorizationKey;
use axom_core::secretbox::SecretBox;
use axom_core::Uuid;
use axom_hr::notify::NotifyConfig;
use axom_hr::PolicyConfig;
use axom_tools::config::ConfigSource;
use axom_tools::mock::MockTools;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

fn jwt(sub: Uuid, aal: &str) -> String {
    let c = json!({ "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

async fn admin(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(id).execute(pool).await.unwrap();
    id
}

fn state(pool: &PgPool, mock: Arc<MockTools>, agent_keys: &[(&str, &str)]) -> AppState {
    let p = policy();
    let services = Services {
        policy: Arc::new(p.clone()),
        notify: NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "https://admin.example.com".into() },
        rooms: mock.clone(),
        mailer: mock.clone(),
        crm: mock,
        livekit_ws_url: "wss://lk.example.com".into(),
        public_api_url: "https://api.example.com".into(),
        meet_web_url: "https://meet.example.com".into(),
        agent_keys: agent_keys.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    };
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    AppState { approvals: Some(approvals), services: Some(Arc::new(services)), ..AppState::new(pool.clone()) }
}

async fn call(app: &axum::Router, method: &str, uri: &str, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn admins_save_keys_encrypted_and_never_see_them_again(pool: PgPool) {
    let sb = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
    let st = AppState { local_integrations: true, secrets: Some(sb.clone()), ..state(&pool, Arc::new(MockTools::new()), &[]) };
    let gen = st.settings_generation.clone();
    let app = router(st);
    let tok = jwt(admin(&pool).await, "aal2");

    let (s, b) = call(&app, "GET", "/admin/integrations", Some(&tok), Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["can_store"], true);
    assert!(b["integrations"].as_array().unwrap().iter().any(|i| i["id"] == "ai"));

    let (s, _) = call(&app, "PUT", "/admin/integrations/DEEPGRAM_API_KEY", Some(&tok), json!({ "value": " dg_secret_value_1234 " })).await;
    assert_eq!(s, StatusCode::OK);
    call(&app, "PUT", "/admin/integrations/SMTP_HOST", Some(&tok), json!({ "value": "smtp.acme.example" })).await;
    assert_eq!(gen.load(Ordering::SeqCst), 2, "each save asks the server to rebuild its adapters");

    // Server keys and bad values are refused; so are sessions without MFA.
    assert_eq!(call(&app, "PUT", "/admin/integrations/AXOM_APPROVAL_SIGNING_KEY", Some(&tok), json!({ "value": "x" })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/admin/integrations/AXOM_MCP_TOKEN", Some(&tok), json!({ "value": "short" })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/admin/integrations/SMTP_HOST", Some(&jwt(Uuid::new_v4(), "aal2")), json!({ "value": "x" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "GET", "/admin/integrations", Some(&jwt(Uuid::new_v4(), "aal1")), Value::Null).await.0, StatusCode::FORBIDDEN);

    let (_, b) = call(&app, "GET", "/admin/integrations", Some(&tok), Value::Null).await;
    let text = b.to_string();
    assert!(!text.contains("dg_secret_value_1234"), "secrets are never returned");
    let dg = b["values"].as_array().unwrap().iter().find(|v| v["name"] == "DEEPGRAM_API_KEY").unwrap();
    assert_eq!(dg["hint"], "••••1234");
    assert!(dg["value"].is_null());
    let host = b["values"].as_array().unwrap().iter().find(|v| v["name"] == "SMTP_HOST").unwrap();
    assert_eq!(host["value"], "smtp.acme.example");

    // Stored encrypted; the server reads them back (trimmed).
    let (ct,): (Vec<u8>,) = sqlx::query_as("SELECT ciphertext FROM axom.integration_settings WHERE name = 'DEEPGRAM_API_KEY'").fetch_one(&pool).await.unwrap();
    assert!(!String::from_utf8_lossy(&ct).contains("dg_secret"));
    let own = axom_api::integrations::load(&pool, Some(&sb)).await.unwrap();
    assert_eq!(own.get("DEEPGRAM_API_KEY").as_deref(), Some("dg_secret_value_1234"));
    // Another key can't read them.
    let other = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
    assert!(axom_api::integrations::load(&pool, Some(&other)).await.unwrap().get("DEEPGRAM_API_KEY").is_none());

    // Audited by name only.
    let events = axom_core::audit::by_action(&pool, "integration.set").await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(!serde_json::to_string(&events).unwrap().contains("dg_secret"));

    let (s, _) = call(&app, "DELETE", "/admin/integrations/DEEPGRAM_API_KEY", Some(&tok), Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    assert!(axom_api::integrations::load(&pool, Some(&sb)).await.unwrap().get("DEEPGRAM_API_KEY").is_none());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn without_a_secrets_key_or_on_the_platform_nothing_is_stored_here(pool: PgPool) {
    let tok = jwt(admin(&pool).await, "aal2");
    let app = router(AppState { local_integrations: true, secrets: None, ..state(&pool, Arc::new(MockTools::new()), &[]) });
    let (_, b) = call(&app, "GET", "/admin/integrations", Some(&tok), Value::Null).await;
    assert_eq!(b["can_store"], false);
    assert!(b["reason"].as_str().unwrap().contains("AXOM_SECRETS_KEY"));
    assert_eq!(call(&app, "PUT", "/admin/integrations/SMTP_HOST", Some(&tok), json!({ "value": "x" })).await.0, StatusCode::SERVICE_UNAVAILABLE);

    // The platform keeps each company's keys itself.
    let app = router(AppState { local_integrations: false, ..state(&pool, Arc::new(MockTools::new()), &[]) });
    assert_eq!(call(&app, "GET", "/admin/integrations", Some(&tok), Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn the_interview_agent_gets_the_company_keys_only_for_a_running_meeting(pool: PgPool) {
    let mock = Arc::new(MockTools::new());
    let app = router(state(&pool, mock.clone(), &[("DEEPGRAM_API_KEY", "dg_company_key")]));
    let tok = jwt(admin(&pool).await, "aal2");
    let (_, m) = call(&app, "POST", "/admin/meetings", Some(&tok), json!({ "title": "Interview", "scheduled_at": "2099-11-10T15:00:00Z", "invitees": ["c@example.com"] })).await;
    let id = m["meeting_id"].as_str().unwrap().to_string();
    let url = format!("/meetings/{id}/callback");
    // Not dispatched yet: no token works.
    assert_eq!(call(&app, "GET", &url, Some("anything"), Value::Null).await.0, StatusCode::UNAUTHORIZED);

    call(&app, "POST", &format!("/admin/meetings/{id}/dispatch"), Some(&tok), json!({ "instructions": "x", "questions": ["Q1?"] })).await;
    // Assigned for 2099: the agent is sent when the candidate joins inside the window.
    assert!(mock.calls_to("livekit").iter().all(|c| c.method != "dispatch_agent"));
    sqlx::query("UPDATE axom.meetings SET scheduled_at = now() WHERE id = $1::uuid").bind(&id).execute(&pool).await.unwrap();
    sqlx::query("UPDATE axom.meeting_invites SET expires_at = now() + interval '10 minutes' WHERE meeting_id = $1::uuid").bind(&id).execute(&pool).await.unwrap();
    let invite = m["join_links"][0]["join_url"].as_str().unwrap().split("t=").nth(1).unwrap().to_string();
    assert_eq!(call(&app, "POST", "/meetings/join", None, json!({ "token": invite })).await.0, StatusCode::OK);
    let meta = mock.calls_to("livekit").into_iter().rev().find(|c| c.method == "dispatch_agent").unwrap().args["metadata"].as_str().unwrap().to_string();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert!(!meta.to_string().contains("dg_company_key"), "keys are never put in the dispatch metadata");
    let agent = meta["callback_token"].as_str().unwrap();

    assert_eq!(call(&app, "GET", &url, None, Value::Null).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", &url, Some("wrong"), Value::Null).await.0, StatusCode::UNAUTHORIZED);
    let (s, b) = call(&app, "GET", &url, Some(agent), Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["keys"], json!({ "DEEPGRAM_API_KEY": "dg_company_key" }));

    // Once the meeting ends, the token no longer returns keys.
    sqlx::query("UPDATE axom.meetings SET status = 'completed' WHERE id = $1::uuid").bind(&id).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "GET", &url, Some(agent), Value::Null).await.0, StatusCode::BAD_REQUEST);
}
