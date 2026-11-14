//! Tools that admins add themselves: MCP servers and HTTP webhooks.
//!
//! - MCP (Model Context Protocol, streamable HTTP): `initialize` (keeping the
//!   `Mcp-Session-Id` the server returns), `notifications/initialized`, then
//!   `tools/list` or `tools/call`. Responses may be plain JSON or a
//!   server-sent event stream.
//! - Webhook: a JSON POST to the admin's URL; the response body is the output.
//!
//! The URLs come from company admins, so every request goes through
//! [`UrlGuard`]: HTTPS only, the host must resolve to public addresses only
//! (no loopback, private, link-local such as cloud metadata, or carrier-NAT
//! ranges), the connection is pinned to the address that was checked (so DNS
//! can't switch to an internal address between check and connect), redirects
//! are not followed, and responses are capped in size. Everything returned is
//! untrusted data.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{ToolError, ToolResult};

/// Largest response accepted from a tool (2 MB).
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Where admin-defined tools may connect.
#[derive(Debug, Clone, Copy, Default)]
pub struct UrlGuard {
    /// Allow plain HTTP and private/loopback addresses. Only for a
    /// single-company deployment reaching its own internal services
    /// (`AXOM_TOOLS_ALLOW_PRIVATE=true`); never on the multi-tenant platform.
    pub allow_private: bool,
}

impl UrlGuard {
    pub fn from_env() -> Self {
        Self { allow_private: std::env::var("AXOM_TOOLS_ALLOW_PRIVATE").as_deref() == Ok("true") }
    }

    /// The platform never allows private addresses, whatever the env says.
    pub fn strict() -> Self {
        Self { allow_private: false }
    }

    /// Check the URL and resolve its host. Returns the URL and the address to
    /// connect to.
    pub async fn check(&self, raw: &str) -> ToolResult<(reqwest::Url, SocketAddr)> {
        let url = reqwest::Url::parse(raw.trim()).map_err(|_| ToolError::Permanent("the URL is not valid".into()))?;
        match url.scheme() {
            "https" => {}
            "http" if self.allow_private => {}
            _ => return Err(ToolError::Permanent("tool URLs must use https".into())),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ToolError::Permanent("put credentials in the token field, not in the URL".into()));
        }
        let host = url.host_str().ok_or_else(|| ToolError::Permanent("the URL has no host".into()))?.to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        let host_for_lookup = host.trim_start_matches('[').trim_end_matches(']').to_string();
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host_for_lookup.as_str(), port))
            .await
            .map_err(|_| ToolError::Retryable(format!("couldn't resolve {host}")))?
            .collect();
        if addrs.is_empty() {
            return Err(ToolError::Retryable(format!("couldn't resolve {host}")));
        }
        if !self.allow_private {
            // The address itself is left out: naming it would let a company
            // map the platform's internal network one hostname at a time.
            if addrs.iter().any(|a| !is_public(a.ip())) {
                return Err(ToolError::Permanent(format!("{host} resolves to a private or reserved address; tools must be on the public internet")));
            }
        }
        Ok((url, addrs[0]))
    }

    pub(crate) async fn client(&self, url: &reqwest::Url, addr: SocketAddr, timeout: Duration) -> ToolResult<reqwest::Client> {
        let mut b = reqwest::Client::builder().timeout(timeout).redirect(reqwest::redirect::Policy::none());
        if let Some(host) = url.host_str() {
            if host.parse::<IpAddr>().is_err() {
                b = b.resolve(host, addr);
            }
        }
        b.build().map_err(|e| ToolError::Permanent(format!("http client: {e}")))
    }
}

/// Whether an address is on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x0064 && s[1] == 0xff9b)) // NAT64 (could reach v4 internals)
                && v6 != Ipv6Addr::UNSPECIFIED
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local() // 169.254/16, including cloud metadata 169.254.169.254
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0 // 0.0.0.0/8
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // carrier-grade NAT 100.64/10
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking 198.18/15
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF 192.0.0/24
        || o[0] >= 240) // reserved
}

/// Authentication header sent with every request (e.g. `Authorization:
/// Bearer <token>`), if any.
#[derive(Clone, Default)]
pub struct ToolAuth(pub Option<(String, String)>);

impl std::fmt::Debug for ToolAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the value.
        write!(f, "ToolAuth({:?})", self.0.as_ref().map(|(h, _)| h))
    }
}

impl ToolAuth {
    fn headers(&self) -> ToolResult<HeaderMap> {
        let mut h = HeaderMap::new();
        if let Some((name, value)) = &self.0 {
            let n = HeaderName::from_bytes(name.as_bytes()).map_err(|_| ToolError::Permanent("bad auth header name".into()))?;
            let mut v = HeaderValue::from_str(value).map_err(|_| ToolError::Permanent("bad auth header value".into()))?;
            v.set_sensitive(true);
            h.insert(n, v);
        }
        Ok(h)
    }
}

/// A tool an MCP server offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

async fn read_capped(resp: reqwest::Response) -> ToolResult<(reqwest::StatusCode, Option<String>, String, Option<String>)> {
    let status = resp.status();
    if status.is_redirection() {
        return Err(ToolError::Permanent(format!("the tool redirected (HTTP {status}); redirects are not followed")));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_RESPONSE_BYTES) {
        return Err(ToolError::Permanent("the tool's response is too large".into()));
    }
    let ctype = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let session = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut body = Vec::new();
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| ToolError::Retryable(format!("reading the tool's response: {}", e.without_url())))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(ToolError::Permanent("the tool's response is too large".into()));
        }
    }
    Ok((status, ctype, String::from_utf8_lossy(&body).into_owned(), session))
}

fn map_status(status: reqwest::StatusCode, body: &str) -> ToolError {
    let snippet: String = body.chars().take(200).collect();
    match status.as_u16() {
        401 | 403 => ToolError::NotAuthorized(format!("the tool refused the credentials (HTTP {status})")),
        429 => ToolError::RateLimited { retry_after_secs: 60 },
        408 | 500..=599 => ToolError::Retryable(format!("HTTP {status}: {snippet}")),
        _ => ToolError::Permanent(format!("HTTP {status}: {snippet}")),
    }
}

/// The JSON-RPC message with this id, from a JSON body or an SSE stream.
pub fn parse_rpc(ctype: Option<&str>, body: &str, id: i64) -> Option<Value> {
    let matches = |v: &Value| v.get("id").and_then(Value::as_i64) == Some(id);
    if ctype.is_some_and(|c| c.starts_with("text/event-stream")) {
        let mut data = String::new();
        let mut found = None;
        for line in body.lines().chain(std::iter::once("")) {
            if let Some(d) = line.strip_prefix("data:") {
                data.push_str(d.trim_start());
            } else if line.is_empty() && !data.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    if matches(&v) {
                        found = Some(v);
                    }
                }
                data.clear();
            }
        }
        found
    } else {
        let v: Value = serde_json::from_str(body).ok()?;
        match v {
            Value::Array(items) => items.into_iter().find(matches),
            v if matches(&v) => Some(v),
            _ => None,
        }
    }
}

/// An MCP server reached over streamable HTTP.
pub struct McpClient {
    client: reqwest::Client,
    url: reqwest::Url,
    auth: HeaderMap,
    session: Option<String>,
    next_id: i64,
}

impl McpClient {
    /// Check the URL, then initialize a session.
    pub async fn connect(guard: &UrlGuard, url: &str, auth: &ToolAuth) -> ToolResult<Self> {
        let (url, addr) = guard.check(url).await?;
        let client = guard.client(&url, addr, Duration::from_secs(30)).await?;
        let mut c = Self { client, url, auth: auth.headers()?, session: None, next_id: 1 };
        let init = c
            .rpc("initialize", json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "axom", "version": env!("CARGO_PKG_VERSION") } }))
            .await?;
        if init.get("protocolVersion").is_none() {
            return Err(ToolError::Permanent("that URL didn't answer like an MCP server".into()));
        }
        // Notification: no id, the server answers 202 with no body.
        let _ = c.post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        Ok(c)
    }

    async fn post(&mut self, body: Value) -> ToolResult<(reqwest::StatusCode, Option<String>, String)> {
        let mut req = self
            .client
            .post(self.url.clone())
            .headers(self.auth.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(&body);
        if let Some(s) = &self.session {
            req = req.header("Mcp-Session-Id", s);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                ToolError::Retryable("the tool timed out".into())
            } else {
                ToolError::Retryable(format!("couldn't reach the tool: {}", e.without_url()))
            }
        })?;
        let (status, ctype, text, session) = read_capped(resp).await?;
        if let Some(s) = session {
            self.session = Some(s);
        }
        Ok((status, ctype, text))
    }

    async fn rpc(&mut self, method: &str, params: Value) -> ToolResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let (status, ctype, text) = self.post(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await?;
        if !status.is_success() {
            return Err(map_status(status, &text));
        }
        let msg = parse_rpc(ctype.as_deref(), &text, id).ok_or_else(|| ToolError::Permanent("the MCP server's answer couldn't be read".into()))?;
        if let Some(err) = msg.get("error") {
            let m = err.get("message").and_then(Value::as_str).unwrap_or("error");
            return Err(match err.get("code").and_then(Value::as_i64) {
                Some(-32601) | Some(-32602) | Some(-32600) => ToolError::Permanent(format!("MCP error: {m}")),
                _ => ToolError::Retryable(format!("MCP error: {m}")),
            });
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// All tools the server offers (following pagination, up to 500).
    pub async fn list_tools(&mut self) -> ToolResult<Vec<McpTool>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let r = self.rpc("tools/list", params).await?;
            let tools: Vec<McpTool> = serde_json::from_value(r.get("tools").cloned().unwrap_or(json!([]))).map_err(|e| ToolError::Permanent(format!("bad tools/list: {e}")))?;
            out.extend(tools);
            cursor = r.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() || out.len() >= 500 {
                break;
            }
        }
        out.truncate(500);
        Ok(out)
    }

    /// Call a tool. Returns the result's structured content, or its text
    /// content joined; a tool-reported error becomes `ToolError::Permanent`.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> ToolResult<Value> {
        let r = self.rpc("tools/call", json!({ "name": name, "arguments": arguments })).await?;
        let text: Vec<String> = r
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string)).collect())
            .unwrap_or_default();
        if r.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(ToolError::Permanent(format!("the tool reported an error: {}", text.join(" ").chars().take(300).collect::<String>())));
        }
        Ok(r.get("structuredContent").cloned().unwrap_or_else(|| Value::String(text.join("\n"))))
    }
}

/// POST JSON to a webhook tool; returns the response body (JSON if it is).
pub async fn call_webhook(guard: &UrlGuard, url: &str, auth: &ToolAuth, body: &Value) -> ToolResult<Value> {
    let (url, addr) = guard.check(url).await?;
    let client = guard.client(&url, addr, Duration::from_secs(30)).await?;
    let resp = client
        .post(url)
        .headers(auth.headers()?)
        .header(ACCEPT, "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| if e.is_timeout() { ToolError::Retryable("the tool timed out".into()) } else { ToolError::Retryable(format!("couldn't reach the tool: {}", e.without_url())) })?;
    let (status, _, text, _) = read_capped(resp).await?;
    if !status.is_success() {
        return Err(map_status(status, &text));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_and_private_addresses() {
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "255.255.255.255", "224.0.0.1",
            "198.18.0.1", "::1", "::", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "64:ff9b::a00:1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn guard_rejects_unsafe_urls() {
        let g = UrlGuard::strict();
        for (url, why) in [
            ("http://example.com/mcp", "https"),
            ("https://127.0.0.1/mcp", "private"),
            ("https://169.254.169.254/latest/meta-data", "private"),
            ("https://[::1]/mcp", "private"),
            ("https://localhost/mcp", "private"),
            ("https://user:pw@example.com/mcp", "credentials"),
            ("ftp://example.com", "https"),
            ("not a url", "valid"),
        ] {
            let e = g.check(url).await.unwrap_err().to_string();
            assert!(e.contains(why), "{url}: {e}");
        }
        assert!(UrlGuard { allow_private: true }.check("http://127.0.0.1:9/x").await.is_ok());
    }

    #[test]
    fn parses_json_and_event_streams() {
        let j = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        assert_eq!(parse_rpc(Some("application/json"), j, 3).unwrap()["result"]["ok"], true);
        assert!(parse_rpc(Some("application/json"), j, 4).is_none());
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\n data: \"result\":{\"tools\":[]}}\n\n";
        // Multi-line data is joined.
        let sse = sse.replace("\n data:", "\ndata:");
        assert_eq!(parse_rpc(Some("text/event-stream"), &sse, 7).unwrap()["result"]["tools"], json!([]));
    }
}
