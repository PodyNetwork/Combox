//! Claude for AI workers: one Messages API call per step. The worker loop
//! (axom-planner `thinking`) decides what to do with each reply; this module
//! only sends requests.
//!
//! [`Llm::create_streaming`] streams the reply and reports Claude's thinking
//! summary and text as they arrive (the dashboard's live agent log), then
//! returns the same message JSON a plain request would.
//!
//! Settings (company integration keys, then the environment):
//! `ANTHROPIC_API_KEY` (required; without it workers don't think) and
//! `AXOM_AI_MODEL` (default `claude-opus-5-5`). The server may set
//! `ANTHROPIC_BASE_URL` to go through a proxy.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::ConfigSource;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
// On a policy decline, the API retries on a suitable model in the same call.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// A streamed reply may take longer than one plain request is allowed to.
const STREAM_TIMEOUT: Duration = Duration::from_secs(1800);

/// What Claude is writing, reported while the reply streams.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A content block opened. `kind` is its type (`thinking`, `text`,
    /// `tool_use`, ...); `name` is the tool's name for a tool call.
    BlockStart { index: usize, kind: String, name: Option<String> },
    /// More thinking summary or reply text for an open block.
    Delta { index: usize, text: String },
    BlockStop { index: usize },
    /// The stream broke and the request is being sent again: what this
    /// attempt reported so far will not be part of the final message.
    Restart,
}

pub type StreamSink = UnboundedSender<StreamEvent>;

/// Sends a Messages API request and returns the response JSON.
#[async_trait]
pub trait Llm: Send + Sync {
    fn model(&self) -> &str;
    async fn create(&self, body: Value) -> anyhow::Result<Value>;

    /// Like [`Llm::create`], but reports thinking and text to `sink` as they
    /// are written. The default sends the request whole and then reports the
    /// finished blocks, for backends that don't stream.
    async fn create_streaming(&self, body: Value, sink: StreamSink) -> anyhow::Result<Value> {
        let resp = self.create(body).await?;
        replay(&resp, &sink);
        Ok(resp)
    }
}

/// Report a finished message's blocks as if they had streamed.
pub fn replay(resp: &Value, sink: &StreamSink) {
    for (index, b) in resp["content"].as_array().into_iter().flatten().enumerate() {
        let kind = b["type"].as_str().unwrap_or_default().to_string();
        let text = match kind.as_str() {
            "thinking" => b["thinking"].as_str(),
            "text" => b["text"].as_str(),
            _ => None,
        };
        let _ = sink.send(StreamEvent::BlockStart { index, kind, name: b["name"].as_str().map(String::from) });
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            let _ = sink.send(StreamEvent::Delta { index, text: t.to_string() });
        }
        let _ = sink.send(StreamEvent::BlockStop { index });
    }
}

pub struct Anthropic {
    http: reqwest::Client,
    key: String,
    model: String,
    url: String,
}

impl Anthropic {
    /// `None` when no Anthropic key is set.
    pub fn from_config(c: &dyn ConfigSource) -> Option<Self> {
        let key = c.get("ANTHROPIC_API_KEY")?;
        let model = c.get("AXOM_AI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into());
        let http = reqwest::Client::builder().timeout(Duration::from_secs(600)).build().ok()?;
        // ANTHROPIC_BASE_URL: a proxy or gateway in front of the API (server setting).
        let url = match std::env::var("ANTHROPIC_BASE_URL").ok().filter(|u| !u.is_empty()) {
            Some(base) => format!("{}/v1/messages", base.trim_end_matches('/')),
            None => API_URL.into(),
        };
        Some(Self { http, key, model, url })
    }

    /// For tests: another endpoint.
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.into();
        self
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        self.http
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .json(body)
    }

    fn prepare(&self, mut body: Value) -> Value {
        body["model"] = json!(self.model);
        body["fallbacks"] = json!("default");
        body
    }
}

/// Rate limits, overload, and server errors are worth another try.
fn retryable(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.as_u16() == 529 || status.is_server_error()
}

async fn api_error(r: reqwest::Response) -> anyhow::Error {
    let status = r.status();
    let text: String = r.text().await.unwrap_or_default().chars().take(500).collect();
    anyhow::anyhow!("Claude API {status}: {text}")
}

#[async_trait]
impl Llm for Anthropic {
    fn model(&self) -> &str {
        &self.model
    }

    async fn create(&self, body: Value) -> anyhow::Result<Value> {
        let body = self.prepare(body);
        // Retry rate limits, overload, and server errors a few times.
        let mut wait = Duration::from_secs(2);
        for attempt in 0..4 {
            match self.request(&body).send().await {
                Ok(r) if r.status().is_success() => return Ok(r.json().await?),
                Ok(r) if attempt < 3 && retryable(r.status()) => {}
                Ok(r) => return Err(api_error(r).await),
                Err(e) if attempt < 3 && (e.is_timeout() || e.is_connect()) => {}
                Err(e) => return Err(e.into()),
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        }
        anyhow::bail!("Claude API unavailable after retries")
    }

    async fn create_streaming(&self, body: Value, sink: StreamSink) -> anyhow::Result<Value> {
        let mut body = self.prepare(body);
        body["stream"] = json!(true);
        let mut wait = Duration::from_secs(2);
        for attempt in 0..4 {
            let again = attempt < 3;
            match self.request(&body).timeout(STREAM_TIMEOUT).send().await {
                Ok(r) if r.status().is_success() => match read_stream(r, &sink).await {
                    Ok(message) => return Ok(message),
                    Err(Fail::Retry(e)) if again => {
                        tracing::warn!(error = %e, "Claude stream broke; sending the request again");
                        let _ = sink.send(StreamEvent::Restart);
                    }
                    Err(Fail::Retry(e) | Fail::Fatal(e)) => return Err(e),
                },
                Ok(r) if again && retryable(r.status()) => {}
                Ok(r) => return Err(api_error(r).await),
                Err(e) if again && (e.is_timeout() || e.is_connect()) => {}
                Err(e) => return Err(e.into()),
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        }
        anyhow::bail!("Claude API unavailable after retries")
    }
}

enum Fail {
    /// Overload or a dropped connection: send the request again.
    Retry(anyhow::Error),
    Fatal(anyhow::Error),
}

/// Read a Messages API event stream, reporting thinking and text to `sink`,
/// and rebuild the message a plain request would have returned: thinking
/// blocks with their signatures, text, and tool calls with parsed input.
async fn read_stream(mut r: reqwest::Response, sink: &StreamSink) -> Result<Value, Fail> {
    let mut asm = Assembler::default();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = r.chunk().await.map_err(|e| Fail::Retry(e.into()))?;
        let Some(chunk) = chunk else { break };
        // Events end with a blank line; JSON payloads never hold a raw CR.
        buf.extend(chunk.iter().filter(|&&b| b != b'\r'));
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let event: Vec<u8> = buf.drain(..end + 2).collect();
            let data: Vec<&str> = std::str::from_utf8(&event)
                .map_err(|e| Fail::Fatal(e.into()))?
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if data.is_empty() {
                continue;
            }
            let data: Value = serde_json::from_str(&data.join("\n")).map_err(|e| Fail::Fatal(anyhow::anyhow!("Claude stream: bad event: {e}")))?;
            if let Some(message) = asm.apply(&data, sink)? {
                return Ok(message);
            }
        }
    }
    Err(Fail::Retry(anyhow::anyhow!("Claude stream ended before the message was complete")))
}

/// Builds the final message from stream events.
#[derive(Default)]
struct Assembler {
    message: Value,
    blocks: Vec<Value>,
    /// Tool input JSON as it arrives, per block.
    partial: Vec<String>,
}

impl Assembler {
    /// Apply one event; returns the message once it is complete.
    fn apply(&mut self, ev: &Value, sink: &StreamSink) -> Result<Option<Value>, Fail> {
        let index = ev["index"].as_u64().unwrap_or(0) as usize;
        match ev["type"].as_str().unwrap_or_default() {
            "message_start" => self.message = ev["message"].clone(),
            "content_block_start" => {
                let block = ev["content_block"].clone();
                if self.blocks.len() <= index {
                    self.blocks.resize(index + 1, Value::Null);
                    self.partial.resize(index + 1, String::new());
                }
                let _ = sink.send(StreamEvent::BlockStart {
                    index,
                    kind: block["type"].as_str().unwrap_or_default().to_string(),
                    name: block["name"].as_str().map(String::from),
                });
                self.blocks[index] = block;
            }
            "content_block_delta" => {
                let Some(block) = self.blocks.get_mut(index) else { return Ok(None) };
                let d = &ev["delta"];
                let text = |s: &str| d[s].as_str().unwrap_or_default().to_string();
                let append = |block: &mut Value, field: &str, more: &str| {
                    let now = block[field].as_str().unwrap_or_default().to_string();
                    block[field] = json!(now + more);
                };
                match d["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let t = text("text");
                        append(block, "text", &t);
                        let _ = sink.send(StreamEvent::Delta { index, text: t });
                    }
                    "thinking_delta" => {
                        let t = text("thinking");
                        append(block, "thinking", &t);
                        let _ = sink.send(StreamEvent::Delta { index, text: t });
                    }
                    "signature_delta" => append(block, "signature", &text("signature")),
                    "input_json_delta" => self.partial[index].push_str(&text("partial_json")),
                    "citations_delta" => {
                        if !block["citations"].is_array() {
                            block["citations"] = json!([]);
                        }
                        if let Some(c) = block["citations"].as_array_mut() {
                            c.push(d["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index) {
                    if block["type"] == "tool_use" {
                        let raw = std::mem::take(&mut self.partial[index]);
                        if !raw.trim().is_empty() {
                            block["input"] = serde_json::from_str(&raw).map_err(|e| Fail::Fatal(anyhow::anyhow!("Claude stream: tool input is not valid JSON: {e}")))?;
                        } else if block["input"].is_null() {
                            block["input"] = json!({});
                        }
                    }
                }
                let _ = sink.send(StreamEvent::BlockStop { index });
            }
            "message_delta" => {
                if let (Some(m), Some(d)) = (self.message.as_object_mut(), ev["delta"].as_object()) {
                    m.extend(d.clone());
                }
                if let Some(u) = ev["usage"].as_object() {
                    if !self.message["usage"].is_object() {
                        self.message["usage"] = json!({});
                    }
                    if let Some(mu) = self.message["usage"].as_object_mut() {
                        mu.extend(u.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())));
                    }
                }
                // Anything else on the event (for example input transformations).
                if let (Some(m), Some(e)) = (self.message.as_object_mut(), ev.as_object()) {
                    m.extend(e.iter().filter(|(k, _)| !matches!(k.as_str(), "type" | "delta" | "usage")).map(|(k, v)| (k.clone(), v.clone())));
                }
            }
            "message_stop" => {
                let mut message = std::mem::take(&mut self.message);
                message["content"] = Value::Array(std::mem::take(&mut self.blocks).into_iter().filter(|b| !b.is_null()).collect());
                return Ok(Some(message));
            }
            "error" => {
                let kind = ev["error"]["type"].as_str().unwrap_or_default();
                let e = anyhow::anyhow!("Claude API stream error ({kind}): {}", ev["error"]["message"].as_str().unwrap_or_default());
                return Err(if matches!(kind, "overloaded_error" | "api_error" | "rate_limit_error") { Fail::Retry(e) } else { Fail::Fatal(e) });
            }
            _ => {} // ping, and event types added later
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapSource;
    use std::sync::{Arc, Mutex};

    #[test]
    fn needs_a_key() {
        assert!(Anthropic::from_config(&MapSource::default()).is_none());
        let c = MapSource([("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string())].into());
        assert_eq!(Anthropic::from_config(&c).unwrap().model(), DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn sends_key_version_model_and_retries_overload() {
        let seen: Arc<Mutex<Vec<(axum::http::HeaderMap, Value)>>> = Arc::default();
        let s = seen.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |h: axum::http::HeaderMap, axum::Json(b): axum::Json<Value>| {
                let s = s.clone();
                async move {
                    let n = {
                        let mut v = s.lock().unwrap();
                        v.push((h, b));
                        v.len()
                    };
                    if n == 1 {
                        (axum::http::StatusCode::from_u16(529).unwrap(), axum::Json(json!({ "type": "error" })))
                    } else {
                        (axum::http::StatusCode::OK, axum::Json(json!({ "stop_reason": "end_turn", "content": [] })))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let c = MapSource([("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string()), ("AXOM_AI_MODEL".to_string(), "claude-sonnet-5-5".to_string())].into());
        let llm = Anthropic::from_config(&c).unwrap().with_url(&format!("http://{addr}/v1/messages"));
        let r = llm.create(json!({ "max_tokens": 10, "messages": [] })).await.unwrap();
        assert_eq!(r["stop_reason"], "end_turn");
        let v = seen.lock().unwrap();
        assert_eq!(v.len(), 2, "retried once after 529");
        let (h, b) = &v[1];
        assert_eq!(h["x-api-key"], "sk-test");
        assert_eq!(h["anthropic-version"], API_VERSION);
        assert_eq!(b["model"], "claude-sonnet-5-5");
        assert_eq!(b["fallbacks"], "default");
    }

    fn sse(events: &[Value]) -> String {
        events.iter().map(|e| format!("event: {}\r\ndata: {}\r\n\r\n", e["type"].as_str().unwrap(), e)).collect()
    }

    fn streamed_reply() -> String {
        sse(&[
            json!({ "type": "message_start", "message": { "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [], "stop_reason": null, "usage": { "input_tokens": 42, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Need the CRM " } }),
            json!({ "type": "ping" }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "list first." } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-abc" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Looking it up." } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "tu_1", "name": "crm__search", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"query\": \"ac" } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "me\"}" } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use", "stop_sequence": null }, "usage": { "output_tokens": 87 } }),
            json!({ "type": "message_stop" }),
        ])
    }

    /// Serves the given bodies in order (status, body) on /v1/messages.
    async fn serve(bodies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let bodies = Arc::new(Mutex::new(bodies.into_iter().rev().collect::<Vec<_>>()));
        let s = seen.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |axum::Json(b): axum::Json<Value>| {
                let (s, bodies) = (s.clone(), bodies.clone());
                async move {
                    s.lock().unwrap().push(b);
                    let (status, body) = bodies.lock().unwrap().pop().unwrap();
                    (axum::http::StatusCode::from_u16(status).unwrap(), [("content-type", "text/event-stream")], body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}/v1/messages"), seen)
    }

    fn client(url: &str) -> Anthropic {
        let c = MapSource([("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string())].into());
        Anthropic::from_config(&c).unwrap().with_url(url)
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[tokio::test]
    async fn streaming_rebuilds_the_message_and_reports_thinking() {
        let (url, seen) = serve(vec![(200, streamed_reply())]).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let m = client(&url).create_streaming(json!({ "max_tokens": 10, "messages": [] }), tx).await.unwrap();

        assert_eq!(seen.lock().unwrap()[0]["stream"], true);
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(m["usage"], json!({ "input_tokens": 42, "output_tokens": 87 }));
        assert_eq!(
            m["content"],
            json!([
                { "type": "thinking", "thinking": "Need the CRM list first.", "signature": "sig-abc" },
                { "type": "text", "text": "Looking it up." },
                { "type": "tool_use", "id": "tu_1", "name": "crm__search", "input": { "query": "acme" } },
            ])
        );
        let events = drain(&mut rx);
        assert_eq!(events[0], StreamEvent::BlockStart { index: 0, kind: "thinking".into(), name: None });
        assert_eq!(events[1], StreamEvent::Delta { index: 0, text: "Need the CRM ".into() });
        assert_eq!(events[2], StreamEvent::Delta { index: 0, text: "list first.".into() });
        assert!(events.contains(&StreamEvent::BlockStart { index: 2, kind: "tool_use".into(), name: Some("crm__search".into()) }));
        assert_eq!(events.last(), Some(&StreamEvent::BlockStop { index: 2 }));
    }

    #[tokio::test]
    async fn streaming_retries_an_overloaded_stream_and_says_so() {
        let broken = sse(&[
            json!({ "type": "message_start", "message": { "id": "msg_0", "content": [], "usage": {} } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "half a thought" } }),
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ]);
        let (url, seen) = serve(vec![(200, broken), (200, streamed_reply())]).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let m = client(&url).create_streaming(json!({ "max_tokens": 10, "messages": [] }), tx).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(m["content"][0]["thinking"], "Need the CRM list first.");
        let events = drain(&mut rx);
        let restart = events.iter().position(|e| *e == StreamEvent::Restart).expect("restart reported");
        assert_eq!(events[restart + 1], StreamEvent::BlockStart { index: 0, kind: "thinking".into(), name: None });
    }

    #[tokio::test]
    async fn streaming_does_not_retry_a_bad_request() {
        let bad = sse(&[json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "nope" } })]);
        let (url, seen) = serve(vec![(200, bad)]).await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let e = client(&url).create_streaming(json!({ "max_tokens": 10, "messages": [] }), tx).await.unwrap_err();
        assert!(e.to_string().contains("invalid_request_error"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn backends_that_do_not_stream_still_report_blocks() {
        struct Plain;
        #[async_trait]
        impl Llm for Plain {
            fn model(&self) -> &str {
                "plain"
            }
            async fn create(&self, _: Value) -> anyhow::Result<Value> {
                Ok(json!({ "content": [{ "type": "thinking", "thinking": "hm", "signature": "s" }, { "type": "text", "text": "" }] }))
            }
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        Plain.create_streaming(json!({}), tx).await.unwrap();
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::BlockStart { index: 0, kind: "thinking".into(), name: None },
                StreamEvent::Delta { index: 0, text: "hm".into() },
                StreamEvent::BlockStop { index: 0 },
                StreamEvent::BlockStart { index: 1, kind: "text".into(), name: None },
                StreamEvent::BlockStop { index: 1 },
            ]
        );
    }
}
