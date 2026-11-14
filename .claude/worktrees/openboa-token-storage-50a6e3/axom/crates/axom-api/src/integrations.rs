//! Single-company deployments: integration settings and API keys set from
//! the dashboard (`/admin/integrations`). Values are encrypted with
//! `AXOM_SECRETS_KEY`, override `.env`, and secrets are never returned, only
//! their last four characters. On the platform these routes are off; the
//! platform stores each company's keys itself.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use axom_core::audit::{self, AuditEvent};
use axom_core::secretbox::SecretBox;
use axom_tools::config::{ConfigSource, EnvSource, MapSource};
use axom_tools::integrations::{validate, CATALOG};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::admin::mfa_admin;
use crate::{reply, AppState};

fn context(name: &str) -> String {
    format!("integration:{name}")
}

/// All stored settings, decrypted. Values that fail to decrypt (for example
/// after AXOM_SECRETS_KEY changed) are skipped and logged.
pub async fn load(pool: &PgPool, secrets: Option<&SecretBox>) -> Result<MapSource, sqlx::Error> {
    let Some(b) = secrets else { return Ok(MapSource::default()) };
    let rows: Vec<(String, Vec<u8>, Vec<u8>)> = sqlx::query_as("SELECT name, nonce, ciphertext FROM axom.integration_settings").fetch_all(pool).await?;
    let mut map = HashMap::new();
    for (name, nonce, ct) in rows {
        match b.open_for(&context(&name), &nonce, &ct) {
            Ok(v) => {
                map.insert(name, v);
            }
            Err(e) => tracing::error!(%name, error = %e, "stored setting could not be decrypted (did AXOM_SECRETS_KEY change?)"),
        }
    }
    Ok(MapSource(map))
}

/// Changes when a setting is saved or removed (to restart the background loop).
pub async fn fingerprint(pool: &PgPool) -> Result<(Option<DateTime<Utc>>, i64), sqlx::Error> {
    sqlx::query_as("SELECT max(updated_at), count(*) FROM axom.integration_settings").fetch_one(pool).await
}

fn off(s: &AppState) -> Option<Response> {
    if s.local_integrations {
        return None;
    }
    Some(reply(StatusCode::NOT_FOUND, json!({ "error": "integrations are managed by the platform for this company" })))
}

pub async fn list(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = off(&s) {
        return r;
    }
    if let Err(r) = mfa_admin(&s, &headers).await {
        return *r;
    }
    let rows: Result<Vec<(String, String, DateTime<Utc>)>, _> =
        sqlx::query_as("SELECT name, hint, updated_at FROM axom.integration_settings ORDER BY name").fetch_all(&s.pool).await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return crate::ApiError(e.to_string()).into_response(),
    };
    let values: Vec<_> = rows
        .into_iter()
        .map(|(name, h, updated_at)| {
            let secret = axom_tools::integrations::field(&name).is_none_or(|f| f.secret);
            json!({ "name": name, "value": (!secret).then(|| h.clone()), "hint": if secret { h } else { String::new() }, "updated_at": updated_at })
        })
        .collect();
    // Names set in the server's .env (values are never shown).
    let server: Vec<&str> = CATALOG.iter().flat_map(|i| i.fields.iter()).map(|f| f.name).filter(|n| EnvSource.get(n).is_some()).collect();
    let can_store = s.secrets.is_some();
    reply(
        StatusCode::OK,
        json!({
            "integrations": CATALOG,
            "values": values,
            "shared": [],
            "server": server,
            "can_store": can_store,
            "live": axom_tools::adapters::http::live_from_env(),
            "reason": (!can_store).then(|| key_problem("to save keys here")),
        }),
    )
}

#[derive(Deserialize)]
pub struct SetValue {
    value: String,
}

pub async fn set(State(s): State<AppState>, headers: HeaderMap, Path(name): Path<String>, Json(b): Json<SetValue>) -> Response {
    if let Some(r) = off(&s) {
        return r;
    }
    let (session, actor) = match mfa_admin(&s, &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(sb) = s.secrets.as_ref() else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": key_problem("to save keys from the dashboard") }));
    };
    let (f, value) = match validate(&name, &b.value) {
        Ok(v) => v,
        Err(m) => return reply(StatusCode::BAD_REQUEST, json!({ "error": m })),
    };
    let (nonce, ct) = sb.seal_for(&context(&name), value);
    let saved = sqlx::query(
        "INSERT INTO axom.integration_settings (name, nonce, ciphertext, hint, updated_by) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (name) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, hint = EXCLUDED.hint,
           updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(&name)
    .bind(nonce)
    .bind(ct)
    .bind(axom_tools::integrations::display(f, value))
    .bind(session.user_id)
    .execute(&s.pool)
    .await;
    if let Err(e) = saved {
        return crate::ApiError(e.to_string()).into_response();
    }
    changed(&s, &actor, "integration.set", &name).await
}

pub async fn delete(State(s): State<AppState>, headers: HeaderMap, Path(name): Path<String>) -> Response {
    if let Some(r) = off(&s) {
        return r;
    }
    let (_, actor) = match mfa_admin(&s, &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(e) = sqlx::query("DELETE FROM axom.integration_settings WHERE name = $1").bind(&name).execute(&s.pool).await {
        return crate::ApiError(e.to_string()).into_response();
    }
    changed(&s, &actor, "integration.delete", &name).await
}

/// Records the change (the setting's name, never its value) and asks the
/// server to rebuild its adapters.
async fn changed(s: &AppState, actor: &axom_core::domain::Actor, action: &str, name: &str) -> Response {
    let ev = AuditEvent::new(actor, action, "ok").inputs(json!({ "name": name }));
    if let Err(e) = audit::append(&s.pool, &ev).await {
        return crate::ApiError(e.to_string()).into_response();
    }
    s.settings_generation.fetch_add(1, Ordering::SeqCst);
    reply(StatusCode::OK, json!({ "saved": name }))
}

/// Why keys can't be saved: the variable is missing, or set but malformed.
fn key_problem(purpose: &str) -> String {
    match axom_core::secretbox::SecretBox::from_env() {
        Err(axom_core::secretbox::CryptoError::BadKey) => format!(
            "AXOM_SECRETS_KEY on the server is not a valid key (it must be exactly 64 hex characters; generate one with `axom secrets-key`). Fix it and restart {purpose}."
        ),
        _ => format!("Set AXOM_SECRETS_KEY on the server (generate one with `axom secrets-key`) {purpose}."),
    }
}
