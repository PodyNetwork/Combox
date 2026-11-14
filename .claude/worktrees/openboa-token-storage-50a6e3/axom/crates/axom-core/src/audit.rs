//! Append-only audit log. The database enforces immutability with triggers;
//! this module only ever inserts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, Row};
use uuid::Uuid;

use crate::domain::Actor;

/// Policy version recorded for events that involve no policy decision.
pub const NO_POLICY: &str = "n/a";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub actor: String,
    pub action: String,
    pub inputs: serde_json::Value,
    pub reasoning_summary: String,
    pub outcome: String,
    pub policy_version: String,
    pub created_at: DateTime<Utc>,
}

impl AuditEvent {
    pub fn new(actor: &Actor, action: impl Into<String>, outcome: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            actor: actor.to_string(),
            action: action.into(),
            inputs: serde_json::Value::Object(Default::default()),
            reasoning_summary: String::new(),
            outcome: outcome.into(),
            policy_version: NO_POLICY.to_string(),
            created_at: Utc::now(),
        }
    }

    pub fn inputs(mut self, inputs: serde_json::Value) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn reasoning(mut self, summary: impl Into<String>) -> Self {
        self.reasoning_summary = summary.into();
        self
    }

    pub fn policy_version(mut self, v: impl Into<String>) -> Self {
        self.policy_version = v.into();
        self
    }

    pub fn at(mut self, at: DateTime<Utc>) -> Self {
        self.created_at = at;
        self
    }
}

/// Append an event. Returns its id.
pub async fn append<'e, E: PgExecutor<'e>>(ex: E, ev: &AuditEvent) -> Result<Uuid, sqlx::Error> {
    sqlx::query(
        "INSERT INTO axom.audit_log
            (id, actor, action, inputs, reasoning_summary, outcome, policy_version, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(ev.id)
    .bind(&ev.actor)
    .bind(&ev.action)
    .bind(&ev.inputs)
    .bind(&ev.reasoning_summary)
    .bind(&ev.outcome)
    .bind(&ev.policy_version)
    .bind(ev.created_at)
    .execute(ex)
    .await?;
    Ok(ev.id)
}

/// Read events for an action, oldest first.
pub async fn by_action<'e, E: PgExecutor<'e>>(ex: E, action: &str) -> Result<Vec<AuditEvent>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, actor, action, inputs, reasoning_summary, outcome, policy_version, created_at
         FROM axom.audit_log WHERE action = $1 ORDER BY seq",
    )
    .bind(action)
    .fetch_all(ex)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(AuditEvent {
                id: r.try_get("id")?,
                actor: r.try_get("actor")?,
                action: r.try_get("action")?,
                inputs: r.try_get("inputs")?,
                reasoning_summary: r.try_get("reasoning_summary")?,
                outcome: r.try_get("outcome")?,
                policy_version: r.try_get("policy_version")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}
