//! DocuSeal (self-hosted on Coolify): contracts from owner-approved templates
//! and tax forms. The adapter maps approved template names to DocuSeal
//! template ids; any other name is refused, so no legal language can be
//! drafted here.
//!
//! API shape used (UNVERIFIED against live docs, see DECISIONS M5-03):
//! base `<DOCUSEAL_URL>/api`, header `X-Auth-Token`;
//! `POST /submissions` {template_id, send_email, submitters[]} -> [submitter{submission_id, ...}];
//! `GET /submissions/{id}` -> {status, submitters[]}; `PUT /submitters/{id}` {send_email:true};
//! `GET /submissions?template_id=&q=<email>` -> {data:[...]}.

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};

use super::http::{live_from_env, need, Auth, HttpAdapter};
use crate::{Authorized, ContractService, Signer, SubmissionStatus, TaxFormKind, ToolError, ToolResult};

pub struct DocuSeal {
    pub http: HttpAdapter,
    /// Approved template name -> DocuSeal template id.
    templates: HashMap<String, i64>,
}

const ACTIONS: &[&str] = &["send_contract", "hire"];

impl DocuSeal {
    pub fn new(base_url: &str, api_key: &str, templates: HashMap<String, i64>, live: bool) -> Self {
        let http = HttpAdapter::new("docuseal", format!("{}/api", base_url.trim_end_matches('/')), Auth::Header { name: "X-Auth-Token", value: api_key.into() }, live, ACTIONS);
        Self { http, templates }
    }

    /// `DOCUSEAL_URL`, `DOCUSEAL_API_KEY`, and `DOCUSEAL_TEMPLATES`
    /// (`name=id,name=id`, names must be on the policy's approved list).
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let mut d = Self::new(&need(c, "DOCUSEAL_URL")?, &need(c, "DOCUSEAL_API_KEY")?, parse_templates(&need(c, "DOCUSEAL_TEMPLATES")?)?, live_from_env());
        if c.tenant_supplied() {
            d.http = d.http.guarded(super::mcp::UrlGuard::strict());
        }
        Ok(d)
    }

    fn template_id(&self, name: &str) -> ToolResult<i64> {
        self.templates
            .get(name)
            .copied()
            .ok_or_else(|| ToolError::NotAuthorized(format!("template {name} is not an owner-approved DocuSeal template")))
    }
}

pub fn parse_templates(spec: &str) -> anyhow::Result<HashMap<String, i64>> {
    spec.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').ok_or_else(|| anyhow::anyhow!("bad DOCUSEAL_TEMPLATES entry {p}"))?;
            Ok((k.trim().to_string(), v.trim().parse()?))
        })
        .collect()
}

#[async_trait]
impl ContractService for DocuSeal {
    async fn create_from_template(&self, auth: &Authorized, template: &str, signers: &[Signer], fields: Value) -> ToolResult<String> {
        self.http.check(auth)?;
        let template_id = self.template_id(template)?;
        let submitters: Vec<Value> = signers.iter().map(|s| json!({ "email": s.email, "name": s.name, "values": fields })).collect();
        let body = json!({ "template_id": template_id, "send_email": false, "submitters": submitters });
        let resp = self.http.send(Method::POST, "/submissions", Some(body), json!([{ "id": 0, "submission_id": 0 }])).await?;
        let id = resp
            .get(0)
            .and_then(|s| s.get("submission_id"))
            .or_else(|| resp.get("id"))
            .and_then(Value::as_i64)
            .ok_or_else(|| ToolError::Permanent("docuseal: no submission_id in response".into()))?;
        Ok(id.to_string())
    }

    async fn send(&self, auth: &Authorized, submission_id: &str) -> ToolResult<()> {
        self.http.check(auth)?;
        let sub = self.http.send(Method::GET, &format!("/submissions/{submission_id}"), None, json!({ "submitters": [{ "id": 0 }] })).await?;
        let submitters = sub.get("submitters").and_then(Value::as_array).cloned().unwrap_or_default();
        for s in submitters {
            if let Some(id) = s.get("id").and_then(Value::as_i64) {
                self.http.send(Method::PUT, &format!("/submitters/{id}"), Some(json!({ "send_email": true })), json!({})).await?;
            }
        }
        Ok(())
    }

    async fn status(&self, submission_id: &str) -> ToolResult<SubmissionStatus> {
        let sub = self.http.send(Method::GET, &format!("/submissions/{submission_id}"), None, json!({ "status": "pending" })).await?;
        let opened = sub
            .get("submitters")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|s| s.get("opened_at").is_some_and(|v| !v.is_null())));
        Ok(match sub.get("status").and_then(Value::as_str).unwrap_or("pending") {
            "completed" => SubmissionStatus::Completed,
            "declined" => SubmissionStatus::Declined,
            _ if opened => SubmissionStatus::Opened,
            "sent" | "awaiting" => SubmissionStatus::Sent,
            _ => SubmissionStatus::Pending,
        })
    }

    async fn fetch_tax_form(&self, signer_email: &str) -> ToolResult<Option<TaxFormKind>> {
        for (name, kind) in [("tax-form-w9", TaxFormKind::W9), ("tax-form-w8ben", TaxFormKind::W8Ben)] {
            let Some(id) = self.templates.get(name) else { continue };
            let q = urlencode(signer_email);
            let resp = self.http.send(Method::GET, &format!("/submissions?template_id={id}&q={q}"), None, json!({ "data": [] })).await?;
            let done = resp
                .get("data")
                .and_then(Value::as_array)
                .is_some_and(|d| d.iter().any(|s| s.get("status").and_then(Value::as_str) == Some("completed")));
            if done {
                return Ok(Some(kind));
            }
        }
        Ok(None)
    }
}

pub(crate) fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
