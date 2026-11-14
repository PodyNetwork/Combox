//! The generic MCP client and webhook caller against a local fake server
//! (session handshake, JSON and event-stream answers, auth, errors).

use axom_tools::adapters::mcp::{call_webhook, McpClient, ToolAuth, UrlGuard};
use axom_tools::ToolError;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

const SESSION: &str = "sess-123";

async fn mcp(headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer tok_abcdef") {
        return (StatusCode::UNAUTHORIZED, "no").into_response();
    }
    let method = req["method"].as_str().unwrap_or("");
    let id = req["id"].clone();
    if method == "initialize" {
        let mut r = Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": "2025-06-18", "capabilities": { "tools": {} }, "serverInfo": { "name": "fake" } } })).into_response();
        r.headers_mut().insert("mcp-session-id", SESSION.parse().unwrap());
        return r;
    }
    // Everything after initialize must carry the session.
    if headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) != Some(SESSION) {
        return (StatusCode::BAD_REQUEST, "missing session").into_response();
    }
    match method {
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
            let page2 = req["params"]["cursor"] == "p2";
            let tools = if page2 {
                json!([{ "name": "search_docs", "description": "Search the docs", "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } }])
            } else {
                json!([{ "name": "create_issue", "description": "Open an issue", "inputSchema": { "type": "object" } }])
            };
            let mut result = json!({ "tools": tools });
            if !page2 {
                result["nextCursor"] = json!("p2");
            }
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
        }
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("");
            if name == "boom" {
                return Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "isError": true, "content": [{ "type": "text", "text": "it broke" }] } })).into_response();
            }
            // Answer as an event stream, with a progress notification first.
            let msg = json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": format!("called {name} with {}", req["params"]["arguments"]) }] } });
            let body = format!("event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\n\nevent: message\ndata: {msg}\n\n");
            ([("content-type", "text/event-stream")], body).into_response()
        }
        _ => Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "no such method" } })).into_response(),
    }
}

async fn hook(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("k_123456") {
        return (StatusCode::FORBIDDEN, "no").into_response();
    }
    Json(json!({ "received": body })).into_response()
}

async fn serve() -> String {
    let app = Router::new()
        .route("/mcp", post(mcp))
        .route("/hook", post(hook))
        .route("/redirect", post(|| async { (StatusCode::FOUND, [("location", "http://169.254.169.254/")]) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

#[tokio::test]
async fn mcp_session_list_and_call() {
    let base = serve().await;
    // Local test server: private addresses allowed only for this test.
    let guard = UrlGuard { allow_private: true };
    let auth = ToolAuth(Some(("Authorization".into(), "Bearer tok_abcdef".into())));

    let mut c = McpClient::connect(&guard, &format!("{base}/mcp"), &auth).await.unwrap();
    let tools = c.list_tools().await.unwrap();
    assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["create_issue", "search_docs"], "follows pagination");
    assert_eq!(tools[1].input_schema["required"][0], "query");

    let out = c.call_tool("search_docs", json!({ "query": "payouts" })).await.unwrap();
    assert_eq!(out, json!("called search_docs with {\"query\":\"payouts\"}"), "event-stream answer, progress skipped");
    assert!(matches!(c.call_tool("boom", json!({})).await, Err(ToolError::Permanent(m)) if m.contains("it broke")));

    // Wrong token: refused at initialize.
    let bad = ToolAuth(Some(("Authorization".into(), "Bearer nope".into())));
    assert!(matches!(McpClient::connect(&guard, &format!("{base}/mcp"), &bad).await, Err(ToolError::NotAuthorized(_))));
    // The strict guard (the platform) refuses the local address outright.
    assert!(McpClient::connect(&UrlGuard::strict(), &format!("{base}/mcp"), &auth).await.is_err());
}

#[tokio::test]
async fn webhook_calls_and_redirects() {
    let base = serve().await;
    let guard = UrlGuard { allow_private: true };
    let auth = ToolAuth(Some(("X-Api-Key".into(), "k_123456".into())));
    let out = call_webhook(&guard, &format!("{base}/hook"), &auth, &json!({ "objective": "x" })).await.unwrap();
    assert_eq!(out["received"]["objective"], "x");
    assert!(matches!(call_webhook(&guard, &format!("{base}/hook"), &ToolAuth::default(), &json!({})).await, Err(ToolError::NotAuthorized(_))));
    // Redirects (here, to cloud metadata) are never followed.
    let e = call_webhook(&guard, &format!("{base}/redirect"), &auth, &json!({})).await.unwrap_err();
    assert!(e.to_string().contains("redirect"), "{e}");
}
