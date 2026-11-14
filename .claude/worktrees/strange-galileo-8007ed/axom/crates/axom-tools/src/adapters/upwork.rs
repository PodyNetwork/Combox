//! Upwork through the provider-hosted freelancing MCP server.
//!
//! The adapter is an MCP client (JSON-RPC 2.0 over streamable HTTP, bearer
//! token): `initialize`, then `tools/call`. The provider's tool names are not
//! documented anywhere we could reach from the build environment, so they are
//! configurable (DECISIONS M8-01):
//! `UPWORK_MCP_TOOL_POST_JOB` (default `post_job`),
//! `UPWORK_MCP_TOOL_LIST_APPLICANTS` (default `list_applicants`),
//! `UPWORK_MCP_TOOL_MESSAGE` (default `message_candidate`).
//! Applicant messages come back wrapped as untrusted data.

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use axom_core::untrusted::Untrusted;
use reqwest::Method;
use serde_json::{json, Value};

use super::http::{live_from_env, need, Auth, HttpAdapter};
use crate::{Applicant, Authorized, FreelanceMarketplace, JobSpec, ToolError, ToolResult};

pub struct ToolNames {
    pub post_job: String,
    pub list_applicants: String,
    pub message: String,
}

impl Default for ToolNames {
    fn default() -> Self {
        Self { post_job: "post_job".into(), list_applicants: "list_applicants".into(), message: "message_candidate".into() }
    }
}

pub struct UpworkMcp {
    pub http: HttpAdapter,
    pub tools: ToolNames,
    next_id: AtomicI64,
}

const ACTIONS: &[&str] = &["hire", "send_external_message"];

impl UpworkMcp {
    pub fn new(mcp_url: &str, token: &str, tools: ToolNames, live: bool) -> Self {
        Self { http: HttpAdapter::new("upwork", mcp_url, Auth::Bearer(token.into()), live, ACTIONS), tools, next_id: AtomicI64::new(1) }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let get = |k: &str, d: &str| c.get(k).unwrap_or_else(|| d.to_string());
        let tools = ToolNames {
            post_job: get("UPWORK_MCP_TOOL_POST_JOB", "post_job"),
            list_applicants: get("UPWORK_MCP_TOOL_LIST_APPLICANTS", "list_applicants"),
            message: get("UPWORK_MCP_TOOL_MESSAGE", "message_candidate"),
        };
        let mut u = Self::new(&need(c, "UPWORK_MCP_URL")?, &need(c, "UPWORK_MCP_TOKEN")?, tools, live_from_env());
        if c.tenant_supplied() {
            u.http = u.http.guarded(super::mcp::UrlGuard::strict());
        }
        Ok(u)
    }

    /// Call a tool and return its result as JSON (parsed from the first text
    /// content block, or `structuredContent` when present).
    async fn call_tool(&self, name: &str, args: Value, dry: Value) -> ToolResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let dry_resp = json!({ "jsonrpc": "2.0", "id": id, "result": { "structuredContent": dry, "isError": false } });
        let resp = self.http.send(Method::POST, "", Some(req), dry_resp).await?;
        if let Some(err) = resp.get("error") {
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("error");
            return Err(match err.get("code").and_then(Value::as_i64) {
                Some(-32602) | Some(-32601) => ToolError::Permanent(format!("upwork mcp: {msg}")),
                _ => ToolError::Retryable(format!("upwork mcp: {msg}")),
            });
        }
        let result = resp.get("result").cloned().unwrap_or(Value::Null);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let text = result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("tool error");
            return Err(ToolError::Permanent(self.http.redactor().clean(&format!("upwork mcp tool {name}: {text}"))));
        }
        if let Some(sc) = result.get("structuredContent") {
            return Ok(sc.clone());
        }
        let text = result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("null");
        Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
    }
}

#[async_trait]
impl FreelanceMarketplace for UpworkMcp {
    async fn post_job(&self, auth: &Authorized, spec: &JobSpec) -> ToolResult<String> {
        self.http.check(auth)?;
        // The job text first, then how the work is measured.
        let description: Vec<&str> = std::iter::once(spec.description.as_str()).chain(spec.kpis.iter().map(String::as_str)).filter(|s| !s.is_empty()).collect();
        let r = self
            .call_tool(&self.tools.post_job, json!({ "title": spec.title, "skills": spec.skills, "hourly_rate_usd": spec.rate_usd, "description": description.join("\n\n") }), json!({ "job_id": "dry-run-job" }))
            .await?;
        r.get("job_id")
            .or_else(|| r.get("id"))
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
            .ok_or_else(|| ToolError::Permanent("upwork mcp: post_job returned no job id".into()))
    }

    async fn list_applicants(&self, job_id: &str) -> ToolResult<Vec<Applicant>> {
        let r = self.call_tool(&self.tools.list_applicants, json!({ "job_id": job_id }), json!({ "applicants": [] })).await?;
        let list = r.get("applicants").or_else(|| r.get("proposals")).and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(list
            .into_iter()
            .filter_map(|a| {
                let id = a.get("id")?.as_str().map(str::to_string).unwrap_or_else(|| a["id"].to_string());
                Some(Applicant {
                    id,
                    name: a.get("name").and_then(Value::as_str).unwrap_or("").chars().take(120).collect(),
                    email: a.get("email").and_then(Value::as_str).map(str::to_string),
                    skills: a.get("skills").and_then(Value::as_array).map(|s| s.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).take(50).collect()).unwrap_or_default(),
                    message: Untrusted::new("upwork_applicant_message", a.get("cover_letter").or_else(|| a.get("message")).and_then(Value::as_str).unwrap_or("")),
                })
            })
            .collect())
    }

    async fn message_candidate(&self, auth: &Authorized, applicant_id: &str, text: &str) -> ToolResult<()> {
        self.http.check(auth)?;
        self.call_tool(&self.tools.message, json!({ "applicant_id": applicant_id, "message": text }), json!({ "ok": true })).await.map(|_| ())
    }
}
