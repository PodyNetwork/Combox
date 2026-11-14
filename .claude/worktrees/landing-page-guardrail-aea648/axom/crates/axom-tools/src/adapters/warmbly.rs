//! Warmbly (hosted sales outreach). We found no public API documentation we
//! could verify from the build environment, so the endpoint layout below is
//! a configurable assumption (DECISIONS M5-04). Base URL `WARMBLY_API_URL`,
//! bearer `WARMBLY_API_KEY`.

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};

use super::http::{live_from_env, need, Auth, HttpAdapter};
use crate::{Authorized, Contact, ReplyStats, SalesOutreach, ToolError, ToolResult};

pub struct Warmbly {
    pub http: HttpAdapter,
}

const ACTIONS: &[&str] = &["send_external_message", "execute_tool"];

impl Warmbly {
    pub fn new(base_url: &str, api_key: &str, live: bool) -> Self {
        Self { http: HttpAdapter::new("warmbly", base_url, Auth::Bearer(api_key.into()), live, ACTIONS) }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let base = c.get("WARMBLY_API_URL").unwrap_or_else(|| "https://api.warmbly.com/v1".into());
        let mut w = Self::new(&base, &need(c, "WARMBLY_API_KEY")?, live_from_env());
        if c.tenant_supplied() {
            w.http = w.http.guarded(super::mcp::UrlGuard::strict());
        }
        Ok(w)
    }
}

fn id_of(v: &Value) -> ToolResult<String> {
    v.get("id")
        .map(|i| i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string()))
        .ok_or_else(|| ToolError::Permanent("warmbly: response has no id".into()))
}

#[async_trait]
impl SalesOutreach for Warmbly {
    async fn create_campaign(&self, auth: &Authorized, name: &str, body: &str) -> ToolResult<String> {
        self.http.check(auth)?;
        id_of(&self.http.send(Method::POST, "/campaigns", Some(json!({ "name": name, "body": body })), json!({ "id": "dry-run" })).await?)
    }
    async fn update_campaign(&self, auth: &Authorized, campaign_id: &str, body: &str) -> ToolResult<()> {
        self.http.check(auth)?;
        self.http.send(Method::PATCH, &format!("/campaigns/{campaign_id}"), Some(json!({ "body": body })), json!({})).await.map(|_| ())
    }
    async fn add_contacts(&self, auth: &Authorized, campaign_id: &str, contacts: &[Contact]) -> ToolResult<u64> {
        self.http.check(auth)?;
        let r = self
            .http
            .send(Method::POST, &format!("/campaigns/{campaign_id}/leads"), Some(json!({ "leads": contacts })), json!({ "added": contacts.len() }))
            .await?;
        Ok(r.get("added").and_then(Value::as_u64).unwrap_or(contacts.len() as u64))
    }
    async fn reply_stats(&self, campaign_id: &str) -> ToolResult<ReplyStats> {
        let r = self.http.send(Method::GET, &format!("/campaigns/{campaign_id}/stats"), None, json!({ "sent": 0, "replies": 0, "positive": 0 })).await?;
        let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
        Ok(ReplyStats { sent: n("sent"), replies: n("replies"), positive: n("positive") })
    }
}
