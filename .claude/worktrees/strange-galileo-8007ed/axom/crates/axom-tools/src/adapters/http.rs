//! Shared plumbing for HTTP adapters: dry-run recording, credential
//! redaction, action checks, and vendor error mapping.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{Authorized, ToolError, ToolResult};

/// Whether adapters make real calls. Dry-run is the default; live mode needs
/// `AXOM_LIVE=true` (after the owner's legal review).
pub fn live_from_env() -> bool {
    std::env::var("AXOM_LIVE").as_deref() == Ok("true")
}

/// A request an adapter made, or would have made in dry-run. Bodies are
/// recorded with secrets redacted.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRequest {
    pub adapter: &'static str,
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
    pub dry_run: bool,
}

/// Replaces every configured secret with `[redacted]`.
#[derive(Clone, Default)]
pub struct Redactor {
    pub(crate) secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        Self { secrets: secrets.into_iter().filter(|s| s.len() >= 4).collect() }
    }

    pub fn clean(&self, text: &str) -> String {
        self.secrets.iter().fold(text.to_string(), |acc, s| acc.replace(s.as_str(), "[redacted]"))
    }
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor({} secrets)", self.secrets.len())
    }
}

/// How the adapter authenticates.
#[derive(Clone)]
pub enum Auth {
    Header { name: &'static str, value: String },
    Bearer(String),
}

/// One vendor endpoint base with credentials, in live or dry-run mode.
#[derive(Clone)]
pub struct HttpAdapter {
    pub name: &'static str,
    base: String,
    auth: Auth,
    live: bool,
    client: reqwest::Client,
    redactor: Redactor,
    log: Arc<Mutex<Vec<RecordedRequest>>>,
    /// Actions (as named in the policy decision) this adapter accepts.
    allowed_actions: &'static [&'static str],
    /// Set when the base URL was typed in by a company on the platform:
    /// every live request is checked and pinned to a public address.
    guard: Option<super::mcp::UrlGuard>,
}

impl std::fmt::Debug for HttpAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print credentials.
        f.debug_struct("HttpAdapter").field("name", &self.name).field("base", &self.base).field("live", &self.live).finish_non_exhaustive()
    }
}

impl HttpAdapter {
    pub fn new(name: &'static str, base: impl Into<String>, auth: Auth, live: bool, allowed_actions: &'static [&'static str]) -> Self {
        let secret = match &auth {
            Auth::Header { value, .. } => value.clone(),
            Auth::Bearer(t) => t.clone(),
        };
        // API base addresses don't redirect; following one would also carry
        // custom auth headers to whatever host it names.
        let client = reqwest::Client::builder().timeout(TIMEOUT).redirect(reqwest::redirect::Policy::none()).build().expect("http client");
        Self {
            name,
            base: base.into().trim_end_matches('/').to_string(),
            auth,
            live,
            client,
            redactor: Redactor::new([secret]),
            log: Arc::new(Mutex::new(Vec::new())),
            allowed_actions,
            guard: None,
        }
    }

    /// Contact the base URL only through `guard` (for addresses a company
    /// supplied on the multi-tenant platform).
    pub fn guarded(mut self, guard: super::mcp::UrlGuard) -> Self {
        self.guard = Some(guard);
        self
    }

    /// Same adapter (shared request log) with a different bearer token, for
    /// services that authenticate each call with a freshly minted token.
    pub fn with_bearer(&self, token: String) -> Self {
        let mut c = self.clone();
        c.redactor.secrets.push(token.clone());
        c.auth = Auth::Bearer(token);
        c
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.log.lock().unwrap().clone()
    }

    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// Refuse unless the policy decision was for an action this adapter serves.
    pub fn check(&self, auth: &Authorized) -> ToolResult<()> {
        if self.allowed_actions.contains(&auth.action()) && !auth.policy_version().is_empty() {
            Ok(())
        } else {
            Err(ToolError::NotAuthorized(format!("{} does not accept a decision for action {}", self.name, auth.action())))
        }
    }

    fn record(&self, method: &str, path: &str, body: Option<&Value>) {
        let body = body.map(|b| serde_json::from_str(&self.redactor.clean(&b.to_string())).unwrap_or(Value::Null));
        self.log.lock().unwrap().push(RecordedRequest {
            adapter: self.name,
            method: method.to_string(),
            path: path.to_string(),
            body,
            dry_run: !self.live,
        });
    }

    /// Send a request. In dry-run nothing leaves the process: the request is
    /// recorded and `dry_response` is returned.
    pub async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>, dry_response: Value) -> ToolResult<Value> {
        self.record(method.as_str(), path, body.as_ref());
        if !self.live {
            return Ok(dry_response);
        }
        let target = format!("{}{path}", self.base);
        let mut req = match &self.guard {
            // Resolve and vet the host now, and connect to exactly that address.
            Some(guard) => {
                let (url, addr) = guard.check(&target).await?;
                guard.client(&url, addr, TIMEOUT).await?.request(method, url)
            }
            None => self.client.request(method, target),
        };
        req = match &self.auth {
            Auth::Header { name, value } => req.header(*name, value),
            Auth::Bearer(t) => req.bearer_auth(t),
        };
        if let Some(b) = &body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() || e.is_connect() {
                ToolError::Retryable(format!("{}: network error", self.name))
            } else {
                ToolError::Permanent(self.redactor.clean(&format!("{}: {}", self.name, e.without_url())))
            }
        })?;
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let text = read_capped(self.name, resp).await?;
        if status.is_success() {
            return Ok(if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) });
        }
        Err(map_status(self.name, status.as_u16(), retry_after, &self.redactor.clean(&text)))
    }
}

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The response body, refusing anything larger than a tool response may be.
async fn read_capped(adapter: &str, mut resp: reqwest::Response) -> ToolResult<String> {
    let max = super::mcp::MAX_RESPONSE_BYTES;
    let too_large = || ToolError::Permanent(format!("{adapter}: the response is too large"));
    if resp.content_length().is_some_and(|n| n as usize > max) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| ToolError::Retryable(format!("{adapter}: network error")))? {
        body.extend_from_slice(&chunk);
        if body.len() > max {
            return Err(too_large());
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Map a vendor HTTP status to a typed error. `body` must already be redacted.
pub fn map_status(adapter: &str, status: u16, retry_after: Option<u64>, body: &str) -> ToolError {
    let snippet: String = body.chars().take(300).collect();
    match status {
        429 => ToolError::RateLimited { retry_after_secs: retry_after.unwrap_or(60) },
        401 | 403 => ToolError::NotAuthorized(format!("{adapter}: HTTP {status} {snippet}")),
        408 | 500..=599 => ToolError::Retryable(format!("{adapter}: HTTP {status} {snippet}")),
        _ => ToolError::Permanent(format!("{adapter}: HTTP {status} {snippet}")),
    }
}

/// Read a required environment variable.
pub fn env(var: &str) -> anyhow::Result<String> {
    need(&crate::config::EnvSource, var)
}

/// Read a required setting from a config source.
pub fn need(c: &dyn crate::config::ConfigSource, var: &str) -> anyhow::Result<String> {
    c.get(var).ok_or_else(|| anyhow::anyhow!("{var} is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(map_status("x", 429, Some(7), ""), ToolError::RateLimited { retry_after_secs: 7 });
        assert!(matches!(map_status("x", 503, None, ""), ToolError::Retryable(_)));
        assert!(matches!(map_status("x", 422, None, ""), ToolError::Permanent(_)));
        assert!(matches!(map_status("x", 401, None, ""), ToolError::NotAuthorized(_)));
    }

    #[test]
    fn redaction() {
        let r = Redactor::new(["sk-very-secret".to_string()]);
        assert_eq!(r.clean("bad key sk-very-secret!"), "bad key [redacted]!");
    }
}
