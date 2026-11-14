//! Requires DATABASE_URL. Admin-defined tools: adding an MCP server (tools
//! discovered, token encrypted) and a webhook, assigning tools to AI workers
//! under policy, running a task with a custom tool (dry-run and live), and
//! cleaning allowlists when tools disappear.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axom_core::db;
use axom_core::domain::{Actor, Capabilities, Task};
use axom_core::secretbox::SecretBox;
use axom_core::state::TaskStatus;
use axom_core::{Decimal, Uuid};
use axom_hr::custom_tools::{self, CustomToolError, CustomToolExecutor, NewServer};
use axom_hr::PolicyConfig;
use axom_planner::runner;
use axom_tools::adapters::mcp::UrlGuard;
use axom_tools::mock::MockTools;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::PgPool;

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

/// A fake MCP server whose tool list can change.
async fn fake_mcp(tools: Arc<Mutex<Vec<&'static str>>>) -> String {
    let app = Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap, Json(req): Json<Value>| {
            let tools = tools.clone();
            async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer gh_secret_token_123") {
                    return (StatusCode::UNAUTHORIZED, "no").into_response();
                }
                let id = req["id"].clone();
                let r: Response = match req["method"].as_str().unwrap_or("") {
                    "initialize" => Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": "2025-06-18", "capabilities": {} } })).into_response(),
                    "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
                    "tools/list" => {
                        let list: Vec<Value> = tools
                            .lock()
                            .unwrap()
                            .iter()
                            .map(|n| json!({ "name": n, "description": format!("{n} tool"), "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } } }))
                            .collect();
                        Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": list } })).into_response()
                    }
                    "tools/call" => Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": format!("{} ran with {}", req["params"]["name"].as_str().unwrap(), req["params"]["arguments"]) }] } })).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                };
                r
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}/mcp")
}

async fn worker(pool: &PgPool, name: &str, kind: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.workers (id, name, kind, role, status, pay_rate_usd, pay_unit) VALUES ($1,$2,$3,'Agent','active',1,'task')")
        .bind(id)
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn assigned_task(pool: &PgPool, worker: Uuid, tool: &str) -> Uuid {
    let task = Task {
        id: Uuid::new_v4(),
        objective: "find the payouts docs".into(),
        required_capabilities: Capabilities::new([tool], Vec::<String>::new()),
        priority: 2,
        deadline: None,
        assignee: None,
        status: TaskStatus::Queued,
        result: None,
        cost_usd: Decimal::ZERO,
        start_at: None,
    };
    db::insert_task(pool, &task, &Default::default()).await.unwrap();
    // Assigned directly (routing is tested elsewhere).
    sqlx::query("UPDATE axom.tasks SET status = 'assigned', assignee = $2 WHERE id = $1").bind(task.id).bind(worker).execute(pool).await.unwrap();
    task.id
}

async fn task_result(pool: &PgPool, id: Uuid) -> (String, Value) {
    let (status, result): (String, Option<Value>) = sqlx::query_as("SELECT status, result FROM axom.tasks WHERE id = $1").bind(id).fetch_one(pool).await.unwrap();
    (status, result.unwrap_or(Value::Null))
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn admins_add_mcp_tools_and_agents_use_them(pool: PgPool) {
    let p = policy();
    let secrets = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
    let guard = UrlGuard { allow_private: true }; // local fake server
    let admin = Actor::Admin { name: "owner".into(), mfa_verified: true };
    let remote = Arc::new(Mutex::new(vec!["search_docs", "create_issue"]));
    let url = fake_mcp(remote.clone()).await;

    // Add an MCP server: its tools are discovered; the token is encrypted.
    let new = |name: &str, kind: &str, url: &str| NewServer {
        name: name.into(),
        kind: kind.into(),
        url: url.into(),
        description: "GitHub".into(),
        auth_header: None,
        auth_value: Some("Bearer gh_secret_token_123".into()),
    };
    let gh = custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &new("github", "mcp", &url)).await.unwrap();
    let (servers, tools) = custom_tools::list(&pool).await.unwrap();
    assert_eq!(servers[0].secret_hint.as_deref(), Some("••••_123"));
    assert_eq!(servers[0].auth_header.as_deref(), Some("Authorization"));
    assert!(servers[0].last_error.is_none());
    assert_eq!(tools.iter().map(|t| t.tool_id.as_str()).collect::<Vec<_>>(), vec!["github.create_issue", "github.search_docs"]);
    let (ct,): (Vec<u8>,) = sqlx::query_as("SELECT secret_ct FROM axom.tool_servers").fetch_one(&pool).await.unwrap();
    assert!(!String::from_utf8_lossy(&ct).contains("gh_secret"), "token stored encrypted");
    let (leaks,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.audit_log WHERE inputs::text LIKE '%gh_secret%'").fetch_one(&pool).await.unwrap();
    assert_eq!(leaks, 0, "the token never reaches the audit log");

    // Names that clash with built-ins or exist already are refused; no key, no token.
    assert!(matches!(custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &new("task_intake", "mcp", &url)).await, Err(CustomToolError::Invalid(_))));
    assert!(matches!(custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &new("github", "mcp", &url)).await, Err(CustomToolError::Invalid(_))));
    assert!(matches!(custom_tools::create_server(&pool, None, &guard, &admin, &new("other", "mcp", &url)).await, Err(CustomToolError::Invalid(m)) if m.contains("AXOM_SECRETS_KEY")));
    assert!(custom_tools::create_server(&pool, Some(&secrets), &UrlGuard::strict(), &admin, &new("local", "webhook", "http://127.0.0.1/x")).await.is_err(), "the platform's guard refuses local URLs");
    // A wrong token is recorded on the server, not hidden.
    let bad = custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &NewServer { auth_value: Some("Bearer wrong".into()), ..new("badtoken", "mcp", &url) }).await.unwrap();
    let (servers, _) = custom_tools::list(&pool).await.unwrap();
    assert!(servers.iter().find(|s| s.id == bad).unwrap().last_error.as_deref().unwrap().contains("refused"));

    // Only an MFA admin can give tools, only to AI workers, only known tools.
    let agent = worker(&pool, "Research Agent", "ai_agent").await;
    let person = worker(&pool, "Dana", "human_freelancer").await;
    let not_mfa = Actor::Admin { name: "owner".into(), mfa_verified: false };
    assert!(matches!(custom_tools::set_worker_tools(&pool, &p, &not_mfa, agent, &["github.search_docs".into()]).await, Err(CustomToolError::Denied(_))));
    assert!(matches!(custom_tools::set_worker_tools(&pool, &p, &Actor::agent("hr"), agent, &["github.search_docs".into()]).await, Err(CustomToolError::Denied(_))));
    assert!(matches!(custom_tools::set_worker_tools(&pool, &p, &admin, person, &["github.search_docs".into()]).await, Err(CustomToolError::Denied(_))));
    assert!(matches!(custom_tools::set_worker_tools(&pool, &p, &admin, agent, &["github.nope".into()]).await, Err(CustomToolError::Invalid(_))));
    let set = custom_tools::set_worker_tools(&pool, &p, &admin, agent, &["github.search_docs".into(), "web_research".into()]).await.unwrap();
    assert_eq!(set, vec!["github.search_docs".to_string(), "web_research".to_string()]);

    // Running a task: dry-run records the call; live calls the MCP server
    // with arguments built from the tool's schema.
    let mocks = MockTools::new();
    let intake = MockTools::new();
    let exec = |live| CustomToolExecutor { pool: pool.clone(), secrets: Some(secrets.clone()), guard, live, fallback: &mocks };
    let t1 = assigned_task(&pool, agent, "github.search_docs").await;
    runner::run_assigned(&pool, &p, &exec(false), &intake).await.unwrap();
    let (status, result) = task_result(&pool, t1).await;
    assert_eq!(status, "done");
    let out = result["tool_outputs"][0]["untrusted_content"].as_str().unwrap();
    assert!(out.starts_with("dry-run: would call MCP tool search_docs"), "{out}");
    let t2 = assigned_task(&pool, agent, "github.search_docs").await;
    runner::run_assigned(&pool, &p, &exec(true), &intake).await.unwrap();
    let (status, result) = task_result(&pool, t2).await;
    assert_eq!(status, "done", "{result}");
    assert_eq!(result["tool_outputs"][0]["untrusted_content"], "search_docs ran with {\"query\":\"find the payouts docs\"}");
    assert_eq!(result["tool_outputs"][0]["source"], "tool:github.search_docs");
    // A tool not on the worker's allowlist blocks the task.
    let t3 = assigned_task(&pool, agent, "github.create_issue").await;
    runner::run_assigned(&pool, &p, &exec(true), &intake).await.unwrap();
    assert_eq!(task_result(&pool, t3).await.0, "blocked");
    // Disabled tools fail instead of running.
    let id: (Uuid,) = sqlx::query_as("SELECT id FROM axom.custom_tools WHERE tool_id = 'github.search_docs'").fetch_one(&pool).await.unwrap();
    custom_tools::set_tool_enabled(&pool, &admin, id.0, false).await.unwrap();
    let t4 = assigned_task(&pool, agent, "github.search_docs").await;
    runner::run_assigned(&pool, &p, &exec(true), &intake).await.unwrap();
    assert_eq!(task_result(&pool, t4).await.0, "failed");
    custom_tools::set_tool_enabled(&pool, &admin, id.0, true).await.unwrap();

    // The server drops a tool: refreshing removes it from allowlists.
    remote.lock().unwrap().retain(|t| *t != "search_docs");
    custom_tools::sync_server(&pool, Some(&secrets), &guard, &admin, gh).await.unwrap();
    let (allow,): (Vec<String>,) = sqlx::query_as("SELECT tool_allowlist FROM axom.workers WHERE id = $1").bind(agent).fetch_one(&pool).await.unwrap();
    assert_eq!(allow, vec!["web_research".to_string()]);

    // Webhooks are one tool named after the server; deleting cleans up.
    let hook = custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &NewServer { auth_value: None, ..new("deploy", "webhook", "http://127.0.0.1:9/hook") }).await.unwrap();
    assert!(custom_tools::available_tools(&pool).await.unwrap().contains(&"deploy".to_string()));
    custom_tools::set_worker_tools(&pool, &p, &admin, agent, &["deploy".into(), "web_research".into()]).await.unwrap();
    custom_tools::delete_server(&pool, &admin, hook).await.unwrap();
    let (allow,): (Vec<String>,) = sqlx::query_as("SELECT tool_allowlist FROM axom.workers WHERE id = $1").bind(agent).fetch_one(&pool).await.unwrap();
    assert_eq!(allow, vec!["web_research".to_string()]);
}

/// A standard tool name (like `crm`) is filled by connecting an MCP server
/// with that name. Until then, tasks that need it wait instead of faking it;
/// once it is connected they resume and use its tools.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn standard_tools_are_filled_by_mcp_servers(pool: PgPool) {
    let p = policy();
    let secrets = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
    let guard = UrlGuard { allow_private: true };
    let admin = Actor::Admin { name: "owner".into(), mfa_verified: true };
    let intake = MockTools::new();
    let unconnected = axom_tools::Unconnected;
    let exec = |live| CustomToolExecutor { pool: pool.clone(), secrets: Some(secrets.clone()), guard, live, fallback: &unconnected };

    // The worker has the standard name `crm` on its list; nothing is connected.
    let agent = worker(&pool, "Sales Agent", "ai_agent").await;
    custom_tools::set_worker_tools(&pool, &p, &admin, agent, &["crm".into()]).await.unwrap();
    let t = assigned_task(&pool, agent, "crm").await;
    runner::run_assigned(&pool, &p, &exec(true), &intake).await.unwrap();
    let (status, result) = task_result(&pool, t).await;
    assert_eq!(status, "blocked");
    assert_eq!(result["waiting_for"], "crm");
    assert!(result["error"].as_str().unwrap().contains("add an MCP server or webhook named crm"));

    // Connect an MCP server named crm with one tool: the task resumes and uses it.
    let url = fake_mcp(Arc::new(Mutex::new(vec!["search_people"]))).await;
    let crm = custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &NewServer {
        name: "crm".into(), kind: "mcp".into(), url, description: "opendesk".into(), auth_header: None, auth_value: Some("Bearer gh_secret_token_123".into()),
    }).await.unwrap();
    runner::run_assigned(&pool, &p, &exec(true), &intake).await.unwrap();
    let (status, result) = task_result(&pool, t).await;
    assert_eq!(status, "done", "{result}");
    assert_eq!(result["tool_outputs"][0]["source"], "tool:crm.search_people");

    // Removing the server keeps `crm` on the worker's list (it waits again).
    custom_tools::delete_server(&pool, &admin, crm).await.unwrap();
    let (allow,): (Vec<String>,) = sqlx::query_as("SELECT tool_allowlist FROM axom.workers WHERE id = $1").bind(agent).fetch_one(&pool).await.unwrap();
    assert_eq!(allow, vec!["crm".to_string()]);
}

struct OneShot(Mutex<Vec<Value>>, Mutex<Vec<Value>>);

#[async_trait::async_trait]
impl axom_tools::llm::Llm for OneShot {
    fn model(&self) -> &str {
        "claude-test"
    }
    async fn create(&self, body: Value) -> anyhow::Result<Value> {
        self.1.lock().unwrap().push(body);
        self.0.lock().unwrap().pop().ok_or_else(|| anyhow::anyhow!("no more replies"))
    }
}

/// A server with several tools needs Claude to choose: without a key the
/// task waits for one; with it, Claude sees every tool of that server.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn several_tools_wait_for_claude_then_all_are_offered(pool: PgPool) {
    let p = policy();
    let secrets = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
    let guard = UrlGuard { allow_private: true };
    let admin = Actor::Admin { name: "owner".into(), mfa_verified: true };
    let intake = MockTools::new();
    let unconnected = axom_tools::Unconnected;
    let exec = CustomToolExecutor { pool: pool.clone(), secrets: Some(secrets.clone()), guard, live: true, fallback: &unconnected };
    let url = fake_mcp(Arc::new(Mutex::new(vec!["search_people", "create_deal"]))).await;
    custom_tools::create_server(&pool, Some(&secrets), &guard, &admin, &NewServer {
        name: "crm".into(), kind: "mcp".into(), url, description: "opendesk".into(), auth_header: None, auth_value: Some("Bearer gh_secret_token_123".into()),
    }).await.unwrap();
    let agent = worker(&pool, "Sales Agent", "ai_agent").await;
    custom_tools::set_worker_tools(&pool, &p, &admin, agent, &["crm".into()]).await.unwrap();
    let t = assigned_task(&pool, agent, "crm").await;

    runner::run_assigned(&pool, &p, &exec, &intake).await.unwrap();
    let (status, result) = task_result(&pool, t).await;
    assert_eq!((status.as_str(), result["waiting_for"].as_str()), ("blocked", Some("ANTHROPIC_API_KEY")));

    let llm = OneShot(
        Mutex::new(vec![
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "Found 2 people." }] }),
            json!({ "stop_reason": "tool_use", "content": [{ "type": "tool_use", "id": "a", "name": "crm__search_people", "input": { "query": "acme" } }] }),
        ]),
        Mutex::default(),
    );
    runner::run_assigned_with(&pool, &p, &exec, &intake, Some(&llm)).await.unwrap();
    let (status, result) = task_result(&pool, t).await;
    assert_eq!(status, "done", "{result}");
    let offered: Vec<String> = llm.1.lock().unwrap()[0]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(offered, ["crm__create_deal", "crm__search_people", "ask_owner", "request_hire"]);
    // Claude's arguments went to the MCP server as given.
    assert_eq!(result["tool_calls"][0]["untrusted_content"], "search_people ran with {\"query\":\"acme\"}");
}
