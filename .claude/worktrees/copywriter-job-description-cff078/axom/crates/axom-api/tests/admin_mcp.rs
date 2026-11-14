//! Requires DATABASE_URL. Admin dashboard endpoints and the MCP server.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::{router, AppState, ApprovalConfig, Services};
use axom_core::authz::AuthorizationKey;
use axom_core::domain::Actor;
use axom_core::Uuid;
use axom_hr::notify::NotifyConfig;
use axom_hr::payouts::{self, PayoutError, PayoutRequest};
use axom_hr::{cap, PolicyConfig, PriceQuote};
use axom_tools::mock::MockTools;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const MCP_TOKEN: &str = "mcp-token-0123456789abcdef0123456789abcdef";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

fn jwt(sub: Uuid, aal: &str) -> String {
    let c = json!({ "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

async fn app(pool: &PgPool) -> (axum::Router, Uuid, Arc<MockTools>) {
    // SAFETY: tests in this file all set the same value.
    unsafe { std::env::set_var("AXOM_MCP_TOKEN", MCP_TOKEN) };
    let p = policy();
    cap::seed_cap(pool, &p).await.unwrap();
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(pool).await.unwrap();
    let mock = Arc::new(MockTools::new());
    let services = Services {
        policy: Arc::new(p.clone()),
        notify: NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "https://admin.example.com".into() },
        rooms: mock.clone(),
        mailer: mock.clone(),
        crm: mock.clone(),
        livekit_ws_url: "wss://lk.example.com".into(),
        public_api_url: "https://api.example.com".into(),
        meet_web_url: "https://meet.example.com".into(),
        agent_keys: Default::default(),
    };
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    let state = AppState { approvals: Some(approvals), services: Some(Arc::new(services)), ..AppState::new(pool.clone()) };
    (router(state), admin, mock)
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

async fn mcp(app: &axum::Router, id: i64, method: &str, params: Value) -> Value {
    call(app, "POST", "/mcp", Some(MCP_TOKEN), json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await.1
}

async fn mcp_audits(pool: &PgPool) -> Vec<String> {
    axom_core::audit::by_action(pool, "mcp.tool_call").await.unwrap().into_iter().map(|e| e.outcome).collect()
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn mcp_requires_token_lists_five_tools_and_audits_every_call(pool: PgPool) {
    let (app, _, mock) = app(&pool).await;
    assert_eq!(call(&app, "POST", "/mcp", None, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "POST", "/mcp", Some("wrong-token-wrong-token-wrong-token-xx"), json!({})).await.0, StatusCode::UNAUTHORIZED);

    let init = mcp(&app, 1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } })).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "axom");
    let list = mcp(&app, 2, "tools/list", json!({})).await;
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["create_meeting", "dispatch_agent", "get_transcript", "get_summary", "send_summary"]);

    // Schema validation rejects bad input, and the rejection is audited.
    let bad = mcp(&app, 3, "tools/call", json!({ "name": "create_meeting", "arguments": { "title": "x", "scheduled_at": "soon", "invitees": ["a@example.com"] } })).await;
    assert_eq!(bad["error"]["code"], -32602);
    // Privileged tools do not exist.
    for tool in ["approve_payout", "set_cap", "kill_switch", "change_policy"] {
        let r = mcp(&app, 4, "tools/call", json!({ "name": tool, "arguments": {} })).await;
        assert_eq!(r["error"]["code"], -32602, "{tool}");
    }

    let created = mcp(&app, 5, "tools/call", json!({ "name": "create_meeting", "arguments": { "title": "Interview", "scheduled_at": "2099-11-10T15:00:00Z", "invitees": ["cand@example.com"] } })).await;
    assert_eq!(created["result"]["isError"], false, "{created}");
    let payload: Value = serde_json::from_str(created["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let room = payload["room_id"].as_str().unwrap().to_string();
    let meeting_id = payload["meeting_id"].as_str().unwrap().to_string();
    assert!(payload["join_links"][0]["join_url"].as_str().unwrap().starts_with("https://meet.example.com/join?t=inv_"));

    let d = mcp(&app, 6, "tools/call", json!({ "name": "dispatch_agent", "arguments": { "room_id": room, "instructions": "Designer interview", "questions": ["Tell me about your portfolio."] } })).await;
    assert_eq!(d["result"]["isError"], false, "{d}");
    // The meeting is far ahead: the agent is assigned now and sent when the candidate joins.
    assert!(d["result"]["content"][0]["text"].as_str().unwrap().contains("joins when the interview opens"), "{d}");
    assert_eq!(mock.calls_to("livekit").iter().filter(|c| c.method == "dispatch_agent").count(), 0);
    let s = mcp(&app, 7, "tools/call", json!({ "name": "send_summary", "arguments": { "meeting_id": meeting_id, "recipients": ["leak@evil.test"] } })).await;
    assert_eq!(s["result"]["isError"], true, "no summary yet / outside recipient");
    let t = mcp(&app, 8, "tools/call", json!({ "name": "get_transcript", "arguments": { "meeting_id": meeting_id } })).await;
    assert!(t["result"]["content"][0]["text"].as_str().unwrap().contains("UNTRUSTED_DATA"));

    // Every tools/call is audited: 1 invalid + 4 unknown + create + dispatch + send + transcript.
    let audits = mcp_audits(&pool).await;
    assert_eq!(audits.len(), 9, "{audits:?}");
    assert_eq!(audits.iter().filter(|o| *o == "invalid").count(), 5);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn kill_switch_halts_tool_calls_and_payouts_immediately(pool: PgPool) {
    let (app, admin, _) = app(&pool).await;
    assert_eq!(call(&app, "POST", "/admin/kill-switch", Some(&jwt(admin, "aal1")), json!({ "on": true })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", "/admin/kill-switch", None, json!({ "on": true })).await.0, StatusCode::UNAUTHORIZED);

    let started = std::time::Instant::now();
    let (st, _) = call(&app, "POST", "/admin/kill-switch", Some(&jwt(admin, "aal2")), json!({ "on": true, "reason": "incident drill" })).await;
    assert_eq!(st, StatusCode::OK);

    // Payouts: denied on the very next attempt.
    let p = policy();
    let quote = PriceQuote { usd_per_near: dec!(4), fetched_at: Utc::now() };
    let req = PayoutRequest { recipient: "w.testnet".into(), amount_usd: dec!(5), ..Default::default() };
    assert!(matches!(payouts::create_payout(&pool, &p, &Actor::System, &req, Some(quote), Utc::now()).await, Err(PayoutError::Denied(_))));
    // Tool calls: an AI worker's queued task is blocked, and no tool runs.
    let w = axom_core::domain::Worker {
        id: Uuid::new_v4(), name: "agent".into(), kind: axom_core::domain::WorkerKind::AiAgent, role: "r".into(),
        status: axom_core::state::WorkerStatus::Active, skills: ["copywriting".to_string()].into(), tool_allowlist: ["postiz".to_string()].into(),
        pay_rate_usd: dec!(1), pay_unit: axom_core::domain::PayUnit::Task, wallet_address: None, kpis: vec![],
    };
    axom_core::db::insert_worker(&pool, None, None, &w).await.unwrap();
    sqlx::query("INSERT INTO axom.tasks (id, objective, required_tools, required_skills, assignee, status) VALUES ($1,'post',ARRAY['postiz'],ARRAY['copywriting'],$2,'assigned')")
        .bind(Uuid::new_v4()).bind(w.id).execute(&pool).await.unwrap();
    let tools = MockTools::new();
    let run = axom_planner::runner::run_assigned(&pool, &p, &tools, &tools).await.unwrap();
    assert_eq!(run.blocked.len(), 1);
    assert!(tools.calls_to("executor").is_empty());
    // MCP tools that act outward are halted too.
    let r = mcp(&app, 1, "tools/call", json!({ "name": "create_meeting", "arguments": { "title": "x", "scheduled_at": "2099-11-10T15:00:00Z", "invitees": ["a@example.com"] } })).await;
    assert_eq!(r["result"]["isError"], true);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "within the time window");

    // The owner is told; turning it off resumes.
    assert_eq!(axom_hr::notify::count_kind(&pool, "kill_switch").await.unwrap(), 1);
    call(&app, "POST", "/admin/kill-switch", Some(&jwt(admin, "aal2")), json!({ "on": false })).await;
    assert!(payouts::create_payout(&pool, &p, &Actor::System, &req, Some(quote), Utc::now()).await.is_ok());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn cap_meter_matches_ledger_and_cap_changes_need_mfa(pool: PgPool) {
    let (app, admin, _) = app(&pool).await;
    let p = policy();
    let now = Utc::now();
    let quote = Some(PriceQuote { usd_per_near: dec!(4), fetched_at: now });
    let mk = |r: &str, a: Decimal| PayoutRequest { recipient: r.into(), amount_usd: a, fee_usd: dec!(0.01), ..Default::default() };
    let auto = payouts::create_payout(&pool, &p, &Actor::System, &mk("a.testnet", dec!(20)), quote, now).await.unwrap();
    payouts::create_payout(&pool, &p, &Actor::System, &mk("b.testnet", dec!(120)), quote, now).await.unwrap(); // pending
    payouts::create_payout(&pool, &p, &Actor::System, &mk("c.testnet", dec!(90)), quote, now).await.unwrap(); // pending
    payouts::mark_sent(&pool, &p, auto.payout_id, now).await.unwrap();
    payouts::create_payout(&pool, &p, &Actor::System, &mk("d.testnet", dec!(400)), quote, now).await.unwrap(); // cap_exceeded

    let (st, meter) = call(&app, "GET", "/admin/cap", Some(&jwt(admin, "aal2")), Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    let ledger = cap::usage(&pool, cap::month_of(now)).await.unwrap();
    let dec_of = |v: &Value| v.as_str().unwrap().parse::<Decimal>().unwrap();
    assert_eq!(dec_of(&meter["committed_usd"]), ledger.committed_usd);
    assert_eq!(dec_of(&meter["reserved_usd"]), ledger.reserved_usd);
    assert_eq!(dec_of(&meter["used_usd"]), ledger.used_usd());
    assert_eq!(dec_of(&meter["committed_usd"]), dec!(20.01));
    assert_eq!(dec_of(&meter["pending_approval_usd"]), dec!(210.02));
    assert_eq!(meter["pending_approval_count"], 2);
    assert_eq!(dec_of(&meter["remaining_usd"]), dec!(500) - dec!(230.03));

    assert_eq!(call(&app, "POST", "/admin/cap", Some(&jwt(admin, "aal1")), json!({ "new_cap_usd": "900", "reason": "x" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", "/admin/cap", Some(&jwt(Uuid::new_v4(), "aal2")), json!({ "new_cap_usd": "900", "reason": "x" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(cap::current_cap(&pool).await.unwrap(), dec!(500));
    assert_eq!(call(&app, "POST", "/admin/cap", Some(&jwt(admin, "aal2")), json!({ "new_cap_usd": "450", "reason": "tighten for November" })).await.0, StatusCode::OK);
    assert_eq!(cap::current_cap(&pool).await.unwrap(), dec!(450));
    let ev = axom_core::audit::by_action(&pool, "cap.change").await.unwrap();
    assert!(ev.iter().any(|e| e.outcome == "applied" && e.actor == "admin:Owner"));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn dashboard_meeting_screens_and_firing_feed(pool: PgPool) {
    let (app, admin, mock) = app(&pool).await;
    let tok = jwt(admin, "aal2");
    let (st, m) = call(&app, "POST", "/admin/meetings", Some(&tok), json!({ "title": "Interview", "scheduled_at": "2099-11-10T15:00:00Z", "invitees": ["c@example.com"] })).await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(m["invites_emailed"], 1);
    let id = m["meeting_id"].as_str().unwrap();
    // Each invitee gets their own join link and a calendar invite.
    let invite = &mock.calls_to("smtp")[0].args["email"];
    assert_eq!(invite["to"][0], "c@example.com");
    assert!(invite["body"].as_str().unwrap().contains(m["join_links"][0]["join_url"].as_str().unwrap()));
    assert!(invite["ics"].as_str().unwrap().contains("BEGIN:VEVENT"));
    let (st, _) = call(&app, "POST", &format!("/admin/meetings/{id}/dispatch"), Some(&tok), json!({ "instructions": "x", "questions": ["Q1?"] })).await;
    assert_eq!(st, StatusCode::OK);
    let (_, list) = call(&app, "GET", "/admin/meetings", Some(&tok), Value::Null).await;
    assert_eq!(list["meetings"][0]["status"], "dispatched");
    let (_, one) = call(&app, "GET", &format!("/admin/meetings/{id}"), Some(&tok), Value::Null).await;
    assert!(one["meeting"].get("callback_token_sha256").is_none(), "no secrets in the dashboard view");

    // The admin can open the room from the dashboard at any time: a fresh link
    // for their own email, which joins like any invite.
    let (st, l) = call(&app, "POST", &format!("/admin/meetings/{id}/join-link"), Some(&tok), Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{l}");
    let url = l["join_url"].as_str().unwrap();
    assert!(url.starts_with("https://meet.example.com/join?t=inv_"));
    let t = url.split("t=").nth(1).unwrap();
    let (st, j) = call(&app, "POST", "/meetings/join", None, json!({ "token": t })).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    assert_eq!(j["identity"], "owner@example.com");
    assert_eq!(call(&app, "POST", &format!("/admin/meetings/{id}/join-link"), Some(&jwt(admin, "aal1")), Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", &format!("/admin/meetings/{}/join-link", Uuid::new_v4()), Some(&tok), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", "/admin/meetings", Some(&jwt(admin, "aal1")), Value::Null).await.0, StatusCode::FORBIDDEN);

    axom_hr::notify::enqueue(&pool, "worker_fired", "Worker fired: X", json!({ "reason": "r" }), None).await.unwrap();
    axom_hr::notify::enqueue(&pool, "cap_exceeded", "Cap", json!({}), None).await.unwrap();
    let (_, feed) = call(&app, "GET", "/admin/notifications?kind=worker_fired", Some(&tok), Value::Null).await;
    assert_eq!(feed["notifications"].as_array().unwrap().len(), 1);
}
