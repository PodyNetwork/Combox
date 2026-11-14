//! Meeting endpoints for participants and the meeting agent.

use axom_hr::meetings::{self, AgentEvent, MeetingError};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{reply, AppState};

pub(crate) fn meeting_error(e: MeetingError) -> Response {
    match e {
        MeetingError::Invalid(m) => reply(StatusCode::BAD_REQUEST, json!({ "error": m })),
        MeetingError::Denied(m) => reply(StatusCode::FORBIDDEN, json!({ "error": m })),
        MeetingError::NotFound(_) => reply(StatusCode::NOT_FOUND, json!({ "error": "meeting not found" })),
        MeetingError::BadToken => reply(StatusCode::UNAUTHORIZED, json!({ "error": "invalid token" })),
        other => crate::ApiError(other.to_string()).into_response(),
    }
}

fn unconfigured() -> Response {
    reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "meetings are not configured" }))
}

#[derive(Deserialize)]
pub struct JoinBody {
    pub token: String,
}

/// Exchange a join-link token for a short-lived LiveKit token.
pub async fn join(State(s): State<AppState>, Json(b): Json<JoinBody>) -> Response {
    let Some(svc) = s.services.clone() else { return unconfigured() };
    match meetings::join(&s.pool, &svc.policy, svc.rooms.as_ref(), &svc.livekit_ws_url, &svc.public_api_url, &b.token).await {
        Ok(info) => reply(StatusCode::OK, json!(info)),
        Err(e) => meeting_error(e),
    }
}

/// The meeting agent reports consent and results (per-meeting bearer token).
pub async fn callback(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>, Json(ev): Json<AgentEvent>) -> Response {
    let token = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    match meetings::record_callback(&s.pool, id, token, ev).await {
        Ok(status) => reply(StatusCode::OK, json!({ "meeting_id": id, "status": status })),
        Err(e) => meeting_error(e),
    }
}

/// The meeting agent fetches the company's own interview AI keys for a
/// meeting it is running (same per-meeting token; only while dispatched).
pub async fn agent_keys(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let token = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if let Err(e) = meetings::check_agent_token(&s.pool, id, token).await {
        return meeting_error(e);
    }
    let keys = s.services.as_ref().map(|svc| svc.agent_keys.clone()).unwrap_or_default();
    let mut r = reply(StatusCode::OK, json!({ "keys": keys }));
    r.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    r
}
