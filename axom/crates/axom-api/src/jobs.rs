//! Public jobs board (Combox Jobs): open jobs and applications. No sign-in;
//! everything submitted is untrusted data (see `axom_hr::board`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axom_hr::board::{self, Application, ApplyError, Cv};
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use crate::{reply, AppState};

/// Applications accepted per minute across the whole API, to blunt floods.
const APPLY_PER_MINUTE: usize = 30;

static RECENT_APPLIES: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

fn under_rate_limit() -> bool {
    let mut recent = RECENT_APPLIES.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    recent.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if recent.len() >= APPLY_PER_MINUTE {
        return false;
    }
    recent.push(now);
    true
}

pub async fn list(State(s): State<AppState>) -> Response {
    match board::open_jobs(&s.pool).await {
        Ok(jobs) => reply(StatusCode::OK, json!({ "jobs": jobs })),
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

pub async fn get(State(s): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match board::open_job(&s.pool, id).await {
        Ok(Some(job)) => reply(StatusCode::OK, json!({ "job": job })),
        Ok(None) => reply(StatusCode::NOT_FOUND, json!({ "error": "job not found or no longer open" })),
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

/// Apply with a CV: a multipart form with an `application` field (the
/// application as JSON) and a `cv` file (a PDF, at most 5 MB).
pub async fn apply(State(s): State<AppState>, Path(id): Path<Uuid>, mut form: Multipart) -> Response {
    let bad = |m: &str| reply(StatusCode::BAD_REQUEST, json!({ "error": m }));
    let mut application: Option<Application> = None;
    let mut cv: Option<Cv> = None;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => return reply(StatusCode::PAYLOAD_TOO_LARGE, json!({ "error": "your CV must be at most 5 MB" })),
            Err(_) => return bad("could not read the form; please try again"),
        };
        match field.name() {
            Some("application") => match field.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()) {
                Some(a) => application = Some(a),
                None => return bad("the application details could not be read"),
            },
            Some("cv") => {
                let filename = field.file_name().unwrap_or("cv.pdf").to_string();
                match field.bytes().await {
                    Ok(b) => cv = Some(Cv { filename, bytes: b.to_vec() }),
                    Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, json!({ "error": "your CV must be at most 5 MB" })),
                }
            }
            _ => {}
        }
    }
    let Some(a) = application else { return bad("the application details are missing") };
    if let Err(e) = board::validate(&a) {
        return bad(&e.to_string());
    }
    let Some(cv) = cv else { return bad("please attach your CV as a PDF") };
    if let Err(e) = board::validate_cv(&cv) {
        return bad(&e.to_string());
    }
    if !under_rate_limit() {
        return reply(StatusCode::TOO_MANY_REQUESTS, json!({ "error": "too many applications right now; please try again in a minute" }));
    }
    match board::apply_with_cv(&s.pool, id, &a, Some(&cv)).await {
        Ok(application_id) => reply(
            StatusCode::CREATED,
            json!({ "application_id": application_id, "next": "We'll email you if you're shortlisted for an interview." }),
        ),
        Err(ApplyError::Invalid(m)) => bad(&m),
        Err(e @ ApplyError::NotOpen) => reply(StatusCode::NOT_FOUND, json!({ "error": e.to_string() })),
        Err(e @ (ApplyError::Duplicate | ApplyError::Full)) => reply(StatusCode::CONFLICT, json!({ "error": e.to_string() })),
        Err(ApplyError::Db) => reply(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "could not save your application; please try again" })),
    }
}
