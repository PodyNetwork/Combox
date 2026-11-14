//! Questions from AI workers, and the replies that let them carry on.
//!
//! A thinking worker that needs something only the people it works for can
//! give (details, a decision, approval to go ahead) calls its `ask_owner`
//! tool. The task is then blocked, waiting for a reply, and its conversation
//! with Claude is kept. An admin's reply sends the task back to the worker,
//! which picks up where it stopped: the reply is the answer to its question.

use axom_core::audit::{self, AuditEvent};
use axom_core::db::{self, transition_in_tx, TransitionDbError};
use axom_core::domain::Actor;
use axom_core::state::TaskStatus;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// `result.waiting_for` of a task waiting for an answer to its questions.
pub const WAITING_FOR_REPLY: &str = "reply";
/// The tool Claude calls to ask.
pub const ASK_TOOL: &str = "ask_owner";
/// Longest reply accepted.
pub const MAX_REPLY_CHARS: usize = 8000;

/// The `ask_owner` tool as offered to Claude.
pub fn ask_tool() -> Value {
    json!({
        "name": ASK_TOOL,
        "description": "Ask the people you work for a question and wait for their answer. Use it when you need something \
            only they can give: missing details, a choice between options, or a go-ahead. The task pauses until they \
            reply, and their reply comes back as this tool's result. Ask everything you need in one go, as a short \
            numbered list. Don't use it for things you can find out with your other tools.",
        "input_schema": {
            "type": "object",
            "properties": { "questions": { "type": "string", "description": "Your questions, as a short numbered list." } },
            "required": ["questions"],
        },
    })
}

/// A conversation waiting for, or resumed with, a reply.
pub struct Thread {
    pub messages: Vec<Value>,
    pub results: Vec<Value>,
    pub calls: Vec<Value>,
    pub ask_id: String,
    pub reply: String,
    pub replied_by: String,
}

/// Keep the conversation of a worker that asked, until someone replies.
pub async fn save(pool: &PgPool, task_id: Uuid, messages: &[Value], results: &[Value], calls: &[Value], ask_id: &str, questions: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO axom.task_threads (task_id, messages, results, calls, ask_id, questions) VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (task_id) DO UPDATE SET messages = $2, results = $3, calls = $4, ask_id = $5, questions = $6,
            reply = NULL, replied_by = NULL, created_at = now(), updated_at = now()",
    )
    .bind(task_id)
    .bind(json!(messages))
    .bind(json!(results))
    .bind(json!(calls))
    .bind(ask_id)
    .bind(questions)
    .execute(pool)
    .await?;
    Ok(())
}

/// Take a replied-to conversation (it is removed: the worker now has it).
pub async fn take_replied(pool: &PgPool, task_id: Uuid) -> sqlx::Result<Option<Thread>> {
    let row: Option<(Value, Value, Value, String, String, Option<String>)> = sqlx::query_as(
        "DELETE FROM axom.task_threads WHERE task_id = $1 AND reply IS NOT NULL RETURNING messages, results, calls, ask_id, reply, replied_by",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(messages, results, calls, ask_id, reply, replied_by)| Thread {
        messages: messages.as_array().cloned().unwrap_or_default(),
        results: results.as_array().cloned().unwrap_or_default(),
        calls: calls.as_array().cloned().unwrap_or_default(),
        ask_id,
        reply,
        replied_by: replied_by.unwrap_or_default(),
    }))
}

/// Whether a task has a kept conversation (waiting for, or with, a reply).
pub async fn has_thread(pool: &PgPool, task_id: Uuid) -> sqlx::Result<bool> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.task_threads WHERE task_id = $1").bind(task_id).fetch_one(pool).await?;
    Ok(n > 0)
}

/// Forget a task's open question (it is being started over).
pub async fn discard(pool: &PgPool, task_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM axom.task_threads WHERE task_id = $1").bind(task_id).execute(pool).await?;
    close_questions(pool, task_id).await
}

async fn close_questions<'e, E: sqlx::PgExecutor<'e>>(ex: E, task_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE axom.agent_log SET done = true, rev = nextval('axom.agent_log_rev'), updated_at = now() WHERE task_id = $1 AND kind = 'question' AND NOT done")
        .bind(task_id)
        .execute(ex)
        .await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ReplyError {
    #[error("task not found")]
    NotFound,
    #[error("this task isn't waiting for a reply")]
    NotWaiting,
    #[error("write a reply first")]
    Empty,
    #[error("the reply is too long (at most {MAX_REPLY_CHARS} characters)")]
    TooLong,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Answer a worker's questions. The task goes back to its worker, which
/// carries on with the reply (on the next task run, within a minute).
pub async fn reply(pool: &PgPool, actor: &Actor, task_id: Uuid, text: &str) -> Result<(), ReplyError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ReplyError::Empty);
    }
    if text.chars().count() > MAX_REPLY_CHARS {
        return Err(ReplyError::TooLong);
    }
    let name = match actor {
        Actor::Admin { name, .. } => name.clone(),
        other => other.to_string(),
    };
    let mut tx = pool.begin().await?;
    let row: Option<(String, Option<String>, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT t.status, t.result->>'waiting_for', t.assignee, w.name
         FROM axom.tasks t LEFT JOIN axom.workers w ON w.id = t.assignee WHERE t.id = $1 FOR UPDATE OF t",
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, waiting_for, assignee, worker_name)) = row else { return Err(ReplyError::NotFound) };
    if status != "blocked" || waiting_for.as_deref() != Some(WAITING_FOR_REPLY) {
        return Err(ReplyError::NotWaiting);
    }
    let saved = sqlx::query("UPDATE axom.task_threads SET reply = $2, replied_by = $3, updated_at = now() WHERE task_id = $1")
        .bind(task_id)
        .bind(text)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    if saved.rows_affected() == 0 {
        return Err(ReplyError::NotWaiting);
    }
    match transition_in_tx::<TaskStatus>(&mut tx, task_id, TaskStatus::Assigned).await {
        Ok(from) => {
            let ev = AuditEvent::new(actor, "task.transition", "applied")
                .inputs(json!({ "table": "axom.tasks", "id": task_id, "from": from.as_str(), "to": "assigned" }))
                .reasoning("replied to the worker's questions");
            audit::append(&mut *tx, &ev).await?;
        }
        Err(TransitionDbError::Invalid(err)) => {
            tx.rollback().await?;
            db::audit_invalid_transition(pool, actor, "axom.tasks", task_id, &err).await?;
            return Err(ReplyError::NotWaiting);
        }
        Err(TransitionDbError::NotFound { .. }) => return Err(ReplyError::NotFound),
        Err(TransitionDbError::Db(e)) => return Err(e.into()),
    }
    sqlx::query("UPDATE axom.tasks SET result = NULL WHERE id = $1").bind(task_id).execute(&mut *tx).await?;
    let ev = AuditEvent::new(actor, "task.reply", "sent").inputs(json!({ "task_id": task_id, "chars": text.chars().count() }));
    audit::append(&mut *tx, &ev).await?;
    close_questions(&mut *tx, task_id).await?;
    sqlx::query("INSERT INTO axom.agent_log (id, task_id, worker_id, worker_name, kind, content, detail) VALUES ($1,$2,$3,$4,'reply',$5,$6)")
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(assignee)
        .bind(worker_name.unwrap_or_default())
        .bind(text)
        .bind(json!({ "from": name }))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
