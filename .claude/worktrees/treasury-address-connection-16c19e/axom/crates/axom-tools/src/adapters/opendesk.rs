//! CRM: opendesk (opendesk.sh, github.com/useopendesk/opendesk), the
//! open-source support inbox. Its REST API (`/v1`, bearer workspace API key
//! `od_live_...` with the `contacts:read` and `contacts:write` scopes) has
//! contacts and companies, each with a free-text `notes` field.
//!
//! - Contacts are found by email (exact, primary or extra address) and
//!   created when missing. An existing contact's name is left alone.
//! - Notes are appended to the contact's notes, dated, never replacing
//!   what's there (opendesk has no separate note records).
//! - opendesk has no deals: a deal is recorded as a note on the contact.
//!
//! `CRM_URL` is the API base: `https://api.opendesk.sh` (the default) or a
//! self-hosted opendesk's API address.

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};

use super::docuseal::urlencode;
use super::http::{live_from_env, need, Auth, HttpAdapter};
use crate::{Authorized, Contact, Crm, ToolError, ToolResult};

pub const DEFAULT_URL: &str = "https://api.opendesk.sh";

pub struct OpendeskCrm {
    pub http: HttpAdapter,
}

const ACTIONS: &[&str] = &["execute_tool", "crm_write"];

impl OpendeskCrm {
    pub fn new(base_url: &str, api_key: &str, live: bool) -> Self {
        Self { http: HttpAdapter::new("crm", format!("{}/v1", base_url.trim_end_matches('/').trim_end_matches("/v1")), Auth::Bearer(api_key.into()), live, ACTIONS) }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&crate::config::EnvSource)
    }

    /// From a config source (a company's stored keys, or the environment).
    /// The URL defaults to opendesk's hosted API; the key is required.
    pub fn from_config(c: &dyn crate::config::ConfigSource) -> anyhow::Result<Self> {
        let key = need(c, "CRM_API_KEY")?;
        let url = c.get("CRM_URL").unwrap_or_else(|| DEFAULT_URL.into());
        anyhow::ensure!(url.starts_with("https://") || url.starts_with("http://"), "CRM_URL must be the opendesk API address (https://...), not {url:?}");
        let mut crm = Self::new(&url, &key, live_from_env());
        if c.tenant_supplied() {
            crm.http = crm.http.guarded(super::mcp::UrlGuard::strict());
        }
        Ok(crm)
    }

    /// The contact with this address (primary or extra), if any.
    async fn find_by_email(&self, email: &str) -> ToolResult<Option<String>> {
        let page = self.http.send(Method::GET, &format!("/contacts?q={}&limit=20", urlencode(email)), None, json!({ "data": [] })).await?;
        let want = email.trim().to_lowercase();
        let same = |v: &Value| v.as_str().is_some_and(|e| e.trim().to_lowercase() == want);
        Ok(page["data"].as_array().into_iter().flatten().find_map(|c| {
            let hit = same(&c["email"]) || c["emails"].as_array().is_some_and(|es| es.iter().any(same));
            hit.then(|| c["id"].as_str().map(str::to_string)).flatten()
        }))
    }

    /// Add a dated entry to a contact's notes, keeping what's there.
    async fn append_note(&self, contact_id: &str, text: &str) -> ToolResult<String> {
        let path = format!("/contacts/{}", urlencode(contact_id));
        let current = self.http.send(Method::GET, &path, None, json!({ "id": contact_id, "notes": "" })).await?;
        let old = current["notes"].as_str().unwrap_or("").trim_end();
        let entry = format!("[{} · Combox]\n{}", chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"), text.trim());
        let notes = if old.is_empty() { entry } else { format!("{old}\n\n{entry}") };
        self.http.send(Method::PATCH, &path, Some(json!({ "notes": notes })), json!({ "id": contact_id })).await?;
        Ok(contact_id.to_string())
    }
}

fn created_id(v: &Value) -> ToolResult<String> {
    v.get("id").or_else(|| v.pointer("/data/id")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| ToolError::Permanent("crm: response has no id".into()))
}

/// A name for a new contact: the given one, or the address's local part
/// when the caller only knows the address.
fn display_name(c: &Contact) -> String {
    let name = c.name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(c.email.trim()) {
        c.email.split('@').next().unwrap_or("").to_string()
    } else {
        name.to_string()
    }
}

#[async_trait]
impl Crm for OpendeskCrm {
    async fn upsert_contact(&self, auth: &Authorized, contact: &Contact) -> ToolResult<String> {
        self.http.check(auth)?;
        if let Some(id) = self.find_by_email(&contact.email).await? {
            return Ok(id);
        }
        let mut body = json!({ "email": contact.email.trim(), "name": display_name(contact) });
        if let Some(company) = contact.company.as_deref().filter(|c| !c.trim().is_empty()) {
            body["custom"] = json!({ "company": company });
        }
        created_id(&self.http.send(Method::POST, "/contacts", Some(body), json!({ "id": "dry-run-contact" })).await?)
    }

    async fn upsert_deal(&self, auth: &Authorized, name: &str, contact_id: &str, value_usd: f64) -> ToolResult<String> {
        self.http.check(auth)?;
        self.append_note(contact_id, &format!("Deal: {name} (USD {value_usd:.2})")).await
    }

    async fn add_note(&self, auth: &Authorized, record_id: &str, note: &str) -> ToolResult<String> {
        self.http.check(auth)?;
        self.append_note(record_id, note).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_contacts_get_a_readable_name() {
        let c = |name: &str| Contact { name: name.into(), email: "ana.silva@example.com".into(), company: None };
        assert_eq!(display_name(&c("Ana Silva")), "Ana Silva");
        assert_eq!(display_name(&c("ana.silva@example.com")), "ana.silva");
        assert_eq!(display_name(&c("")), "ana.silva");
    }

    #[test]
    fn the_base_is_the_v1_api_either_way() {
        let a = OpendeskCrm::new("https://api.opendesk.sh", "k", false);
        let b = OpendeskCrm::new("https://api.opendesk.sh/v1/", "k", false);
        assert_eq!(format!("{:?}", a.http), format!("{:?}", b.http));
        assert!(format!("{:?}", a.http).contains("https://api.opendesk.sh/v1"));
    }
}
