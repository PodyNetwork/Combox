//! Postiz (self-hosted on Coolify): schedule posts across channels.
//! API shape used (UNVERIFIED against live docs, DECISIONS M5-05):
//! `POST <POSTIZ_URL>/public/v1/posts`, header `Authorization: <api key>`,
//! body {type:"schedule", date, shortLink, tags, posts:[{integration:{id}, value:[{content}], settings}]}.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::{json, Value};

use super::http::{live_from_env, need, Auth, HttpAdapter};
use crate::{Authorized, SocialScheduler, ToolResult};

pub struct Postiz {
    pub http: HttpAdapter,
}

const ACTIONS: &[&str] = &["send_external_message", "execute_tool"];

impl Postiz {
    pub fn new(base_url: &str, api_key: &str, live: bool) -> Self {
        let base = format!("{}/public/v1", base_url.trim_end_matches('/'));
        Self { http: HttpAdapter::new("postiz", base, Auth::Header { name: "Authorization", value: api_key.into() }, live, ACTIONS) }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let mut p = Self::new(&need(c, "POSTIZ_URL")?, &need(c, "POSTIZ_API_KEY")?, live_from_env());
        if c.tenant_supplied() {
            p.http = p.http.guarded(super::mcp::UrlGuard::strict());
        }
        Ok(p)
    }
}

#[async_trait]
impl SocialScheduler for Postiz {
    async fn schedule_post(&self, auth: &Authorized, channels: &[String], content: &str, at: DateTime<Utc>) -> ToolResult<String> {
        self.http.check(auth)?;
        let posts: Vec<Value> = channels
            .iter()
            .map(|c| json!({ "integration": { "id": c }, "value": [{ "content": content, "image": [] }], "settings": {} }))
            .collect();
        let body = json!({ "type": "schedule", "date": at.to_rfc3339(), "shortLink": false, "tags": [], "posts": posts });
        let r = self.http.send(Method::POST, "/posts", Some(body), json!([{ "postId": "dry-run" }])).await?;
        Ok(r.get(0)
            .and_then(|p| p.get("postId").or_else(|| p.get("id")))
            .or_else(|| r.get("id"))
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
            .unwrap_or_else(|| "scheduled".into()))
    }
}
