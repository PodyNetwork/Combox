//! Tools that a company's admins add themselves, instead of only the
//! built-in ones: MCP servers (their tools are discovered) and HTTP webhooks.
//!
//! - Adding, refreshing, enabling, and removing tools needs an MFA-verified
//!   admin (enforced by the API) and is audited.
//! - An AI worker can use a tool only once an MFA admin puts it on the
//!   worker's allowlist (`ChangeToolAllowlist`, a protected policy action).
//!   People never get tools.
//! - Every call is checked by the policy engine (`ExecuteTool`), runs in
//!   dry-run unless `AXOM_LIVE=true`, and its output is untrusted data.
//! - URLs pass the URL guard (public HTTPS only); tokens are encrypted with
//!   `AXOM_SECRETS_KEY`, bound to the tool's row, and never returned.

use async_trait::async_trait;
use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::secretbox::SecretBox;
use axom_core::untrusted::Untrusted;
use axom_tools::adapters::mcp::{call_webhook, McpClient, ToolAuth, UrlGuard};
use axom_tools::{Authorized, ToolError, ToolExecutor, ToolResult, ToolSpec};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::policy::{self, Action, Context, PolicyConfig};

/// The standard tool names (slots). A company fills one by adding an MCP
/// server or webhook with that name; workers with the name on their list
/// then use that server's tools.
pub const BUILTIN_TOOLS: &[&str] = axom_tools::STANDARD_TOOLS;

#[derive(Debug, thiserror::Error)]
pub enum CustomToolError {
    #[error("{0}")]
    Invalid(String),
    #[error("not allowed: {0}")]
    Denied(String),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewServer {
    pub name: String,
    /// "mcp" or "webhook".
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    /// Header for the token, default "Authorization".
    #[serde(default)]
    pub auth_header: Option<String>,
    /// Full header value, e.g. "Bearer sk-..." (encrypted at rest).
    #[serde(default)]
    pub auth_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ServerView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub description: String,
    pub auth_header: Option<String>,
    pub secret_hint: Option<String>,
    pub enabled: bool,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ToolView {
    pub id: Uuid,
    pub server_id: Uuid,
    pub tool_id: String,
    pub remote_name: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub enabled: bool,
}

fn aad(server: Uuid) -> String {
    format!("axom-tool:{server}")
}

fn hint(v: &str) -> String {
    let n = v.chars().count();
    if n <= 8 {
        "••••".into()
    } else {
        format!("••••{}", v.chars().skip(n - 4).collect::<String>())
    }
}

async fn audit_ev(pool: &PgPool, actor: &Actor, action: &str, outcome: &str, inputs: Value) -> Result<(), sqlx::Error> {
    audit::append(pool, &AuditEvent::new(actor, action, outcome).inputs(inputs)).await.map(|_| ())
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    (2..=31).contains(&name.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// MCP tool names become `<server>.<name>`; characters outside
/// [A-Za-z0-9_-.] make the tool unusable here (skipped).
fn tool_id(server: &str, remote: &str) -> Option<String> {
    let ok = !remote.is_empty() && remote.len() <= 64 && remote.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    ok.then(|| format!("{server}.{remote}"))
}

pub async fn list(pool: &PgPool) -> Result<(Vec<ServerView>, Vec<ToolView>), sqlx::Error> {
    let servers = sqlx::query_as::<_, ServerView>(
        "SELECT id, name, kind, url, description, auth_header, secret_hint, enabled, last_synced_at, last_error, created_by, created_at
         FROM axom.tool_servers ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let tools = sqlx::query_as::<_, ToolView>("SELECT id, server_id, tool_id, remote_name, description, input_schema, enabled FROM axom.custom_tools ORDER BY tool_id")
        .fetch_all(pool)
        .await?;
    Ok((servers, tools))
}

/// Built-in tools plus enabled custom tools on enabled servers.
pub async fn available_tools(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let custom: Vec<(String,)> = sqlx::query_as(
        "SELECT t.tool_id FROM axom.custom_tools t JOIN axom.tool_servers s ON s.id = t.server_id WHERE t.enabled AND s.enabled ORDER BY t.tool_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(BUILTIN_TOOLS.iter().map(|s| s.to_string()).chain(custom.into_iter().map(|r| r.0)).collect())
}

/// Add a tool server. MCP servers are contacted to discover their tools
/// (an admin action, allowed in dry-run; tool *calls* stay dry-run).
pub async fn create_server(pool: &PgPool, secrets: Option<&SecretBox>, guard: &UrlGuard, actor: &Actor, b: &NewServer) -> Result<Uuid, CustomToolError> {
    let name = b.name.trim().to_lowercase();
    if !valid_name(&name) {
        return Err(CustomToolError::Invalid("name: 2 to 31 characters, starting with a letter; lowercase letters, digits, - and _".into()));
    }
    if name == "task_intake" {
        return Err(CustomToolError::Invalid("that name is reserved; choose another".into()));
    }
    if !matches!(b.kind.as_str(), "mcp" | "webhook") {
        return Err(CustomToolError::Invalid("kind must be mcp or webhook".into()));
    }
    if b.description.chars().count() > 500 || b.url.len() > 2000 {
        return Err(CustomToolError::Invalid("description up to 500 characters; URL up to 2000".into()));
    }
    guard.check(&b.url).await?;
    let value = b.auth_value.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let header = match (&b.auth_header, value) {
        (_, None) => None,
        (Some(h), Some(_)) if !h.trim().is_empty() => Some(h.trim().to_string()),
        (_, Some(_)) => Some("Authorization".to_string()),
    };
    if let Some(h) = &header {
        if !h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || h.len() > 64 {
            return Err(CustomToolError::Invalid("the auth header name looks wrong".into()));
        }
    }
    if value.is_some_and(|v| v.len() > 4000 || v.chars().any(char::is_control)) {
        return Err(CustomToolError::Invalid("the token is too long or has line breaks".into()));
    }
    let id = Uuid::new_v4();
    let sealed = match value {
        Some(v) => {
            let b = secrets.ok_or_else(|| CustomToolError::Invalid("storing tool tokens needs AXOM_SECRETS_KEY to be set on the server".into()))?;
            let (n, ct) = b.seal_for(&aad(id), v);
            Some((n, ct, hint(v)))
        }
        None => None,
    };
    let inserted = sqlx::query(
        "INSERT INTO axom.tool_servers (id, name, kind, url, description, auth_header, secret_nonce, secret_ct, secret_hint, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (name) DO NOTHING",
    )
    .bind(id)
    .bind(&name)
    .bind(&b.kind)
    .bind(b.url.trim())
    .bind(b.description.trim())
    .bind(&header)
    .bind(sealed.as_ref().map(|s| s.0.clone()))
    .bind(sealed.as_ref().map(|s| s.1.clone()))
    .bind(sealed.as_ref().map(|s| s.2.clone()))
    .bind(actor.to_string())
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(CustomToolError::Invalid("a tool with that name already exists".into()));
    }
    audit_ev(pool, actor, "tool_server.create", "created", json!({ "server_id": id, "name": name, "kind": b.kind, "url": b.url.trim() })).await?;
    if b.kind == "webhook" {
        sqlx::query("INSERT INTO axom.custom_tools (id, server_id, tool_id, description) VALUES ($1,$2,$3,$4)")
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(&name)
            .bind(b.description.trim())
            .execute(pool)
            .await?;
    } else {
        // Discovery failures are recorded on the server (fix and refresh).
        let _ = sync_server(pool, secrets, guard, actor, id).await;
    }
    Ok(id)
}

struct ServerRow {
    name: String,
    kind: String,
    url: String,
    auth: ToolAuth,
}

async fn load_server(pool: &PgPool, secrets: Option<&SecretBox>, id: Uuid) -> Result<ServerRow, CustomToolError> {
    type Row = (String, String, String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>);
    let r: Option<Row> = sqlx::query_as("SELECT name, kind, url, auth_header, secret_nonce, secret_ct FROM axom.tool_servers WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let (name, kind, url, header, nonce, ct) = r.ok_or(CustomToolError::NotFound)?;
    let auth = match (header, nonce, ct) {
        (Some(h), Some(n), Some(c)) => {
            let b = secrets.ok_or_else(|| CustomToolError::Invalid("AXOM_SECRETS_KEY is not set, so the tool's token can't be read".into()))?;
            let v = b.open_for(&aad(id), &n, &c).map_err(|e| CustomToolError::Invalid(e.to_string()))?;
            ToolAuth(Some((h, v)))
        }
        _ => ToolAuth(None),
    };
    Ok(ServerRow { name, kind, url, auth })
}

/// Re-discover an MCP server's tools: new ones are added (enabled), gone
/// ones removed (and taken off workers' allowlists).
pub async fn sync_server(pool: &PgPool, secrets: Option<&SecretBox>, guard: &UrlGuard, actor: &Actor, id: Uuid) -> Result<usize, CustomToolError> {
    let s = load_server(pool, secrets, id).await?;
    if s.kind != "mcp" {
        return Err(CustomToolError::Invalid("only MCP servers are refreshed".into()));
    }
    let listed = async {
        let mut c = McpClient::connect(guard, &s.url, &s.auth).await?;
        c.list_tools().await
    }
    .await;
    let tools = match listed {
        Ok(t) => t,
        Err(e) => {
            sqlx::query("UPDATE axom.tool_servers SET last_error = $2 WHERE id = $1").bind(id).bind(e.to_string()).execute(pool).await?;
            audit_ev(pool, actor, "tool_server.sync", "error", json!({ "server_id": id, "error": e.to_string() })).await?;
            return Err(e.into());
        }
    };
    let mut keep = Vec::new();
    for t in &tools {
        let Some(tid) = tool_id(&s.name, &t.name) else { continue };
        let description: String = t.description.chars().take(1000).collect();
        sqlx::query(
            "INSERT INTO axom.custom_tools (id, server_id, tool_id, remote_name, description, input_schema) VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (tool_id) DO UPDATE SET description = EXCLUDED.description, input_schema = EXCLUDED.input_schema, updated_at = now()",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&tid)
        .bind(&t.name)
        .bind(&description)
        .bind(if t.input_schema.is_object() { t.input_schema.clone() } else { json!({}) })
        .execute(pool)
        .await?;
        keep.push(tid);
    }
    let gone: Vec<(String,)> = sqlx::query_as("DELETE FROM axom.custom_tools WHERE server_id = $1 AND NOT (tool_id = ANY($2)) RETURNING tool_id")
        .bind(id)
        .bind(&keep)
        .fetch_all(pool)
        .await?;
    for (g,) in gone.iter().filter(|g| !BUILTIN_TOOLS.contains(&g.0.as_str())) {
        sqlx::query("UPDATE axom.workers SET tool_allowlist = array_remove(tool_allowlist, $1)").bind(g).execute(pool).await?;
    }
    sqlx::query("UPDATE axom.tool_servers SET last_synced_at = now(), last_error = NULL WHERE id = $1").bind(id).execute(pool).await?;
    audit_ev(pool, actor, "tool_server.sync", "ok", json!({ "server_id": id, "tools": keep, "removed": gone.iter().map(|g| &g.0).collect::<Vec<_>>() })).await?;
    Ok(keep.len())
}

pub async fn set_server_enabled(pool: &PgPool, actor: &Actor, id: Uuid, enabled: bool) -> Result<(), CustomToolError> {
    let n = sqlx::query("UPDATE axom.tool_servers SET enabled = $2 WHERE id = $1").bind(id).bind(enabled).execute(pool).await?.rows_affected();
    if n == 0 {
        return Err(CustomToolError::NotFound);
    }
    audit_ev(pool, actor, "tool_server.enable", if enabled { "enabled" } else { "disabled" }, json!({ "server_id": id })).await?;
    Ok(())
}

pub async fn set_tool_enabled(pool: &PgPool, actor: &Actor, id: Uuid, enabled: bool) -> Result<(), CustomToolError> {
    let n = sqlx::query("UPDATE axom.custom_tools SET enabled = $2, updated_at = now() WHERE id = $1").bind(id).bind(enabled).execute(pool).await?.rows_affected();
    if n == 0 {
        return Err(CustomToolError::NotFound);
    }
    audit_ev(pool, actor, "tool.enable", if enabled { "enabled" } else { "disabled" }, json!({ "tool": id })).await?;
    Ok(())
}

/// Remove a server and its tools, and take them off every allowlist.
pub async fn delete_server(pool: &PgPool, actor: &Actor, id: Uuid) -> Result<(), CustomToolError> {
    let mut tx = pool.begin().await?;
    let tools: Vec<(String,)> = sqlx::query_as("SELECT tool_id FROM axom.custom_tools WHERE server_id = $1").bind(id).fetch_all(&mut *tx).await?;
    // A standard name stays on workers' lists: they wait until it is connected again.
    for (t,) in tools.iter().filter(|t| !BUILTIN_TOOLS.contains(&t.0.as_str())) {
        sqlx::query("UPDATE axom.workers SET tool_allowlist = array_remove(tool_allowlist, $1)").bind(t).execute(&mut *tx).await?;
    }
    let n = sqlx::query("DELETE FROM axom.tool_servers WHERE id = $1").bind(id).execute(&mut *tx).await?.rows_affected();
    if n == 0 {
        return Err(CustomToolError::NotFound);
    }
    audit::append(&mut *tx, &AuditEvent::new(actor, "tool_server.delete", "deleted").inputs(json!({ "server_id": id, "tools": tools.iter().map(|t| &t.0).collect::<Vec<_>>() }))).await?;
    tx.commit().await?;
    Ok(())
}

/// Replace an AI worker's tool allowlist (MFA admin only, through the
/// policy engine). Every tool must exist; people never get tools.
pub async fn set_worker_tools(pool: &PgPool, cfg: &PolicyConfig, actor: &Actor, worker_id: Uuid, tools: &[String]) -> Result<Vec<String>, CustomToolError> {
    let d = policy::evaluate(cfg, &Action::ChangeToolAllowlist { worker_id }, &Context::new(actor.clone(), Utc::now()));
    if d.outcome != axom_core::decision::Outcome::Allow {
        return Err(CustomToolError::Denied(d.reason));
    }
    let row: Option<(String, Vec<String>)> = sqlx::query_as("SELECT kind, tool_allowlist FROM axom.workers WHERE id = $1").bind(worker_id).fetch_optional(pool).await?;
    let (kind, before) = row.ok_or(CustomToolError::NotFound)?;
    let mut tools: Vec<String> = tools.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    tools.sort();
    tools.dedup();
    if kind != "ai_agent" && !tools.is_empty() {
        return Err(CustomToolError::Denied("people never get tools; only AI workers do".into()));
    }
    let available = available_tools(pool).await?;
    if let Some(unknown) = tools.iter().find(|t| !available.contains(t)) {
        return Err(CustomToolError::Invalid(format!("unknown or disabled tool: {unknown}")));
    }
    sqlx::query("UPDATE axom.workers SET tool_allowlist = $2 WHERE id = $1").bind(worker_id).bind(&tools).execute(pool).await?;
    let ev = AuditEvent::new(actor, "worker.tool_allowlist", "changed").inputs(json!({ "worker_id": worker_id, "before": before, "after": tools })).policy_version(&d.policy_version);
    audit::append(pool, &ev).await?;
    Ok(tools)
}

/// Arguments for an MCP tool from a task. When a thinking worker chose
/// them (`input.arguments`), they are used as given. Otherwise string
/// properties that look like the request (objective, query, prompt, ...) get
/// the task objective, and `task_id` gets the task id; others are left out.
pub fn arguments_for(schema: &Value, input: &Value) -> Value {
    if let Some(args) = input.get("arguments").filter(|a| a.is_object()) {
        return args.clone();
    }
    let objective = input.get("objective").cloned().unwrap_or(Value::Null);
    let mut args = Map::new();
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (k, p) in props {
            let is_string = p.get("type").and_then(Value::as_str).is_none_or(|t| t == "string");
            let key = k.to_lowercase();
            if key == "task_id" {
                if let Some(t) = input.get("task_id") {
                    args.insert(k.clone(), json!(t.to_string().trim_matches('"')));
                }
            } else if is_string && ["objective", "query", "prompt", "input", "text", "task", "description", "message", "q", "question", "instructions"].contains(&key.as_str()) {
                args.insert(k.clone(), objective.clone());
            }
        }
    }
    Value::Object(args)
}

/// Runs admin-defined tools; anything else goes to `fallback` (the built-in
/// tools). Dry-run unless `live`: the call is recorded and not made.
pub struct CustomToolExecutor<'a> {
    pub pool: PgPool,
    pub secrets: Option<SecretBox>,
    pub guard: UrlGuard,
    pub live: bool,
    pub fallback: &'a dyn ToolExecutor,
}

#[async_trait]
impl ToolExecutor for CustomToolExecutor<'_> {
    /// A tool id gives that tool; a server name (such as a standard name
    /// like `crm`) gives every enabled tool of that server. Anything else is
    /// up to the fallback.
    async fn resolve(&self, name: &str) -> ToolResult<Option<Vec<ToolSpec>>> {
        type Row = (String, String, Value);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT t.tool_id, t.description, t.input_schema FROM axom.custom_tools t JOIN axom.tool_servers s ON s.id = t.server_id
             WHERE t.enabled AND s.enabled AND (t.tool_id = $1 OR s.name = $1) ORDER BY t.tool_id",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ToolError::Retryable(format!("tool registry: {e}")))?;
        if rows.is_empty() {
            return self.fallback.resolve(name).await;
        }
        Ok(Some(
            rows.into_iter()
                .map(|(id, description, schema)| {
                    if schema.get("properties").is_some_and(Value::is_object) {
                        ToolSpec { id, description, input_schema: schema }
                    } else {
                        ToolSpec::simple(&id, &description)
                    }
                })
                .collect(),
        ))
    }

    async fn execute(&self, auth: &Authorized, tool: &str, input: Value) -> ToolResult<Untrusted> {
        type Row = (Uuid, Option<String>, Value, bool, bool);
        let row: Option<Row> = sqlx::query_as(
            "SELECT t.server_id, t.remote_name, t.input_schema, t.enabled, s.enabled FROM axom.custom_tools t JOIN axom.tool_servers s ON s.id = t.server_id WHERE t.tool_id = $1",
        )
        .bind(tool)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ToolError::Retryable(format!("tool registry: {e}")))?;
        let Some((server_id, remote, schema, tool_on, server_on)) = row else {
            return self.fallback.execute(auth, tool, input).await;
        };
        if auth.action() != "execute_tool" || auth.policy_version().is_empty() {
            return Err(ToolError::NotAuthorized(format!("{tool} needs an execute_tool authorization")));
        }
        if !tool_on || !server_on {
            return Err(ToolError::Permanent(format!("{tool} is disabled")));
        }
        let s = load_server(&self.pool, self.secrets.as_ref(), server_id).await.map_err(|e| ToolError::Permanent(e.to_string()))?;
        let source = format!("tool:{tool}");
        if !self.live {
            let what = match &remote {
                Some(r) => format!("dry-run: would call MCP tool {r} on {} with {}", s.url, arguments_for(&schema, &input)),
                None => format!("dry-run: would POST the task to {}", s.url),
            };
            return Ok(Untrusted::new(source, what));
        }
        let out = match remote {
            Some(r) => {
                let mut c = McpClient::connect(&self.guard, &s.url, &s.auth).await?;
                c.call_tool(&r, arguments_for(&schema, &input)).await?
            }
            None => call_webhook(&self.guard, &s.url, &s.auth, &json!({ "tool": tool, "task": input })).await?,
        };
        Ok(Untrusted::new(source, match out {
            Value::String(t) => t,
            other => other.to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_arguments() {
        assert!(valid_name("github") && valid_name("my-crm_2"));
        assert!(!valid_name("G") && !valid_name("9lives") && !valid_name("has space") && !valid_name(&"x".repeat(32)));
        assert_eq!(tool_id("github", "create_issue").as_deref(), Some("github.create_issue"));
        assert_eq!(tool_id("github", "bad name"), None);
        let schema = json!({ "type": "object", "properties": { "query": { "type": "string" }, "limit": { "type": "integer" }, "task_id": { "type": "string" } } });
        let args = arguments_for(&schema, &json!({ "task_id": "abc", "objective": "find docs" }));
        assert_eq!(args, json!({ "query": "find docs", "task_id": "abc" }));
        assert_eq!(arguments_for(&json!({}), &json!({ "objective": "x" })), json!({}));
        let chosen = json!({ "query": "rust mcp", "limit": 3 });
        assert_eq!(arguments_for(&schema, &json!({ "objective": "x", "arguments": chosen.clone() })), chosen);
    }
}
