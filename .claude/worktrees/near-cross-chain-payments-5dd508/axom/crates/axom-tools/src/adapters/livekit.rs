//! LiveKit (self-hosted on Coolify, never LiveKit Cloud).
//!
//! Access tokens are LiveKit's documented JWT format (HS256 signed with the
//! API secret, `iss` = API key, `sub` = identity, `video` grants), minted
//! here with short expiry. There is no official Rust server SDK we could
//! pin, so this replaces the SDK's token builder (DECISIONS M6-01). Room
//! management uses LiveKit's Twirp API: `POST /twirp/livekit.RoomService/CreateRoom`,
//! `/DeleteRoom`, and agent dispatch via `livekit.AgentDispatchService/CreateDispatch`.

use async_trait::async_trait;
use chrono::Utc;
use jsonwebtoken::{encode, EncodingKey, Header};
use reqwest::Method;
use serde_json::{json, Value};

use super::http::{need, Auth, HttpAdapter};
use crate::{Authorized, MeetingRooms, RoomInfo, ToolError, ToolResult};

pub struct LiveKit {
    pub http: HttpAdapter,
    /// `wss://` URL participants connect to.
    pub ws_url: String,
    api_key: String,
    api_secret: String,
    /// Participant token lifetime in seconds (short by design).
    pub participant_ttl_secs: i64,
}

impl std::fmt::Debug for LiveKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKit").field("ws_url", &self.ws_url).finish_non_exhaustive()
    }
}

const ACTIONS: &[&str] = &["create_meeting", "dispatch_agent", "join_meeting", "end_meeting"];

impl LiveKit {
    pub fn new(ws_url: &str, api_key: &str, api_secret: &str, live: bool) -> Self {
        let http_url = ws_url.replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
        let mut http = HttpAdapter::new("livekit", http_url, Auth::Bearer(String::new()), live, ACTIONS);
        http = http.with_bearer(api_secret.to_string());
        Self { http, ws_url: ws_url.into(), api_key: api_key.into(), api_secret: api_secret.into(), participant_ttl_secs: 15 * 60 }
    }

    /// The LiveKit server bundled with Combox (server settings, never a
    /// company's): `LIVEKIT_URL` (what browsers connect to),
    /// `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, and optionally
    /// `LIVEKIT_API_URL` when Combox reaches the server at another address
    /// (inside Docker, `http://livekit:7880`).
    ///
    /// Always live, whatever `AXOM_LIVE` says (DECISIONS M6-16): this is
    /// Combox's own server, and the participant tokens it hands out are real
    /// either way, so a dry run only left people in real rooms the
    /// interviewer was never sent to. Emails, contracts, and payouts stay
    /// behind `AXOM_LIVE`.
    pub fn from_env() -> anyhow::Result<Self> {
        let c = crate::config::EnvSource;
        let mut lk = Self::new(&need(&c, "LIVEKIT_URL")?, &need(&c, "LIVEKIT_API_KEY")?, &need(&c, "LIVEKIT_API_SECRET")?, true);
        if let Some(api) = crate::config::ConfigSource::get(&c, "LIVEKIT_API_URL") {
            lk.http = HttpAdapter::new("livekit", api, Auth::Bearer(String::new()), true, ACTIONS).with_bearer(lk.api_secret.clone());
        }
        Ok(lk)
    }

    /// Mint a LiveKit access token.
    pub fn token(&self, identity: &str, name: &str, ttl_secs: i64, video: Value, metadata: Option<&str>) -> ToolResult<String> {
        let now = Utc::now().timestamp();
        let mut claims = json!({ "iss": self.api_key, "sub": identity, "name": name, "nbf": now, "exp": now + ttl_secs, "video": video });
        if let Some(m) = metadata {
            claims["metadata"] = json!(m);
        }
        encode(&Header::default(), &claims, &EncodingKey::from_secret(self.api_secret.as_bytes()))
            .map_err(|e| ToolError::Permanent(format!("livekit token: {e}")))
    }

    fn server_token(&self, room: Option<&str>) -> ToolResult<String> {
        let mut video = json!({ "roomCreate": true, "roomList": true, "roomAdmin": true, "agent": true });
        if let Some(r) = room {
            video["room"] = json!(r);
        }
        self.token("axom-backend", "Combox", 60, video, None)
    }

    async fn twirp(&self, service: &str, method: &str, room: Option<&str>, body: Value, dry: Value) -> ToolResult<Value> {
        let http = self.http.with_bearer(self.server_token(room)?);
        http.send(Method::POST, &format!("/twirp/livekit.{service}/{method}"), Some(body), dry).await
    }

}

#[async_trait]
impl MeetingRooms for LiveKit {
    async fn create_room(&self, auth: &Authorized, name: &str) -> ToolResult<RoomInfo> {
        self.http.check(auth)?;
        self.twirp("RoomService", "CreateRoom", None, json!({ "name": name, "empty_timeout": 600, "max_participants": 10 }), json!({ "name": name })).await?;
        Ok(RoomInfo { room: name.into(), url: self.ws_url.clone() })
    }

    async fn participant_token(&self, auth: &Authorized, room: &str, identity: &str) -> ToolResult<String> {
        self.http.check(auth)?;
        let video = json!({ "room": room, "roomJoin": true, "canPublish": true, "canSubscribe": true, "canPublishData": true });
        self.token(identity, identity, self.participant_ttl_secs, video, None)
    }

    /// Dispatch the meeting agent to a room (LiveKit Agents explicit dispatch).
    async fn dispatch_agent(&self, auth: &Authorized, room: &str, agent_name: &str, metadata: &str) -> ToolResult<String> {
        self.http.check(auth)?;
        let r = self
            .twirp("AgentDispatchService", "CreateDispatch", Some(room), json!({ "room": room, "agent_name": agent_name, "metadata": metadata }), json!({ "id": "dry-run-dispatch" }))
            .await?;
        Ok(r.get("id").and_then(Value::as_str).unwrap_or("dispatched").to_string())
    }

    /// Lists the room's participants (`RoomService/ListParticipants`) and
    /// looks for a connected agent with this name. A room LiveKit no longer
    /// has (closed when it sat empty) has no agent in it.
    async fn agent_in_room(&self, auth: &Authorized, room: &str, agent_name: &str) -> ToolResult<bool> {
        self.http.check(auth)?;
        // Dry run: report the agent as present, so nothing is sent twice.
        let dry = json!({ "participants": [{ "identity": "agent-dry-run", "kind": "AGENT", "state": "ACTIVE", "attributes": { "lk.agent.name": agent_name } }] });
        match self.twirp("RoomService", "ListParticipants", Some(room), json!({ "room": room }), dry).await {
            Ok(r) => Ok(has_agent(&r, agent_name)),
            Err(ToolError::Permanent(e)) if e.contains("HTTP 404") => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn end_room(&self, auth: &Authorized, room: &str) -> ToolResult<()> {
        self.http.check(auth)?;
        self.twirp("RoomService", "DeleteRoom", Some(room), json!({ "room": room }), json!({})).await.map(|_| ())
    }
}

/// Whether a `ListParticipants` response holds a connected agent with this
/// name. LiveKit marks agents with `kind: "AGENT"` and the
/// `lk.agent.name` attribute; one that is leaving (`DISCONNECTED`) does not count.
fn has_agent(list: &Value, agent_name: &str) -> bool {
    list.get("participants").and_then(Value::as_array).is_some_and(|ps| {
        ps.iter().any(|p| {
            let agent = p.get("kind").and_then(Value::as_str) == Some("AGENT") || p.pointer("/permission/agent").and_then(Value::as_bool) == Some(true);
            let attrs = p.get("attributes");
            let name = attrs.and_then(|a| a.get("lk.agent.name").or_else(|| a.get("lk.agent_name"))).and_then(Value::as_str);
            let leaving = p.get("state").and_then(Value::as_str) == Some("DISCONNECTED");
            agent && !leaving && name.is_none_or(|n| n == agent_name)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axom_core::decision::{Decision, Outcome};
    use jsonwebtoken::{decode, DecodingKey, Validation};

    #[test]
    fn finds_the_agent_in_a_participant_list() {
        // The shape LiveKit v1.13 returns (trimmed), checked against the local server.
        let list = json!({ "participants": [
            { "identity": "cand@example.com", "kind": "STANDARD", "state": "ACTIVE", "attributes": {} },
            { "identity": "agent-AJ_1", "kind": "AGENT", "state": "ACTIVE", "permission": { "agent": true }, "attributes": { "lk.agent.name": "axom-interviewer" } },
        ] });
        assert!(has_agent(&list, "axom-interviewer"));
        assert!(!has_agent(&list, "another-agent"));
        assert!(!has_agent(&json!({ "participants": [{ "identity": "cand@example.com", "kind": "STANDARD" }] }), "axom-interviewer"));
        assert!(!has_agent(&json!({ "participants": [{ "identity": "agent-AJ_1", "kind": "AGENT", "state": "DISCONNECTED", "attributes": { "lk.agent.name": "axom-interviewer" } }] }), "axom-interviewer"));
        assert!(!has_agent(&json!({}), "axom-interviewer"), "an empty room");
    }

    #[tokio::test]
    async fn participant_token_is_short_lived_and_scoped() {
        let lk = LiveKit::new("wss://lk.example.com", "APIkey", "secret-secret-secret-secret-1234", false);
        let d = Decision { outcome: Outcome::Allow, reason: "".into(), policy_version: "v".into(), notify_owner: false };
        let auth = Authorized::from_decision("join_meeting", &d).unwrap();
        let t = lk.participant_token(&auth, "room-1", "cand@example.com").await.unwrap();
        let mut v = Validation::default();
        v.validate_aud = false;
        let c = decode::<Value>(&t, &DecodingKey::from_secret(b"secret-secret-secret-secret-1234"), &v).unwrap().claims;
        assert_eq!(c["iss"], "APIkey");
        assert_eq!(c["sub"], "cand@example.com");
        assert_eq!(c["video"]["room"], "room-1");
        assert_eq!(c["video"]["roomJoin"], true);
        assert!(c["video"].get("roomCreate").is_none(), "participants cannot create rooms");
        let ttl = c["exp"].as_i64().unwrap() - c["nbf"].as_i64().unwrap();
        assert!(ttl <= 15 * 60);
        // Dry-run room creation makes no call.
        let a = Authorized::from_decision("create_meeting", &d).unwrap();
        lk.create_room(&a, "room-1").await.unwrap();
        assert!(lk.http.requests().iter().all(|r| r.dry_run && r.path == "/twirp/livekit.RoomService/CreateRoom"));
        assert!(!format!("{lk:?}").contains("secret-secret"));
    }
}
