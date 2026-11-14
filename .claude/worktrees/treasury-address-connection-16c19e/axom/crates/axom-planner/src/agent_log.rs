//! The live agent log: what AI workers are thinking and doing, written to
//! `axom.agent_log` while they work so the dashboard can follow along.
//!
//! Thinking summaries and reply text are written as Claude streams them
//! (at most a few writes a second per entry); tool calls and status changes
//! are written when they happen. The log is for watching, not the record:
//! a failed write is logged and skipped, never fails the task. The audit log
//! stays the record.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axom_tools::llm::StreamEvent;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

/// How often an entry that is still streaming is written.
const FLUSH_EVERY: Duration = Duration::from_millis(400);
/// Entries older than this are deleted.
const KEEP_DAYS: i32 = 14;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub id: Uuid,
    pub rev: i64,
    pub task_id: Uuid,
    pub task_objective: Option<String>,
    pub worker_id: Option<Uuid>,
    pub worker_name: String,
    /// `status`, `thinking`, `text`, or `tool`.
    pub kind: String,
    pub content: String,
    pub detail: Value,
    /// False while Claude is still writing it.
    pub done: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Entries written or changed after `after` (a `rev`), oldest change first;
/// with no `after`, the latest `limit` entries.
pub async fn since(pool: &PgPool, after: Option<i64>, limit: i64) -> sqlx::Result<Vec<Entry>> {
    const COLS: &str = "l.id, l.rev, l.task_id, t.objective AS task_objective, l.worker_id, l.worker_name, l.kind, l.content, l.detail, l.done, l.created_at, l.updated_at
         FROM axom.agent_log l LEFT JOIN axom.tasks t ON t.id = l.task_id";
    match after {
        Some(rev) => sqlx::query_as(&format!("SELECT {COLS} WHERE l.rev > $1 ORDER BY l.rev LIMIT $2")).bind(rev).bind(limit).fetch_all(pool).await,
        None => {
            let mut rows: Vec<Entry> = sqlx::query_as(&format!("SELECT {COLS} ORDER BY l.rev DESC LIMIT $1")).bind(limit).fetch_all(pool).await?;
            rows.reverse();
            Ok(rows)
        }
    }
}

/// Delete entries older than two weeks.
pub async fn prune(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query("DELETE FROM axom.agent_log WHERE created_at < now() - make_interval(days => $1)").bind(KEEP_DAYS).execute(pool).await?;
    Ok(r.rows_affected())
}

/// Writes one worker's entries for one task.
pub struct AgentLog<'a> {
    pool: &'a PgPool,
    task_id: Uuid,
    worker_id: Uuid,
    worker_name: String,
}

/// A thinking or text block Claude is still writing.
struct Open {
    id: Option<Uuid>,
    kind: &'static str,
    text: String,
    written: usize,
}

impl<'a> AgentLog<'a> {
    pub fn new(pool: &'a PgPool, task_id: Uuid, worker_id: Uuid, worker_name: &str) -> Self {
        Self { pool, task_id, worker_id, worker_name: worker_name.to_string() }
    }

    /// A status change: started, finished, blocked, failed.
    pub async fn status(&self, state: &str, content: impl Into<String>) {
        self.insert("status", &content.into(), json!({ "state": state }), true).await;
    }

    /// A question for the people the worker works for; open until answered.
    pub async fn question(&self, questions: &str) {
        self.insert("question", questions, json!({}), false).await;
    }

    /// A tool call and how it went.
    pub async fn tool(&self, tool: &str, arguments: &Value, outcome: &str, note: Option<&str>) {
        let content = match note {
            Some(n) => format!("{tool}: {n}"),
            None => tool.to_string(),
        };
        self.insert("tool", &content, json!({ "tool": tool, "arguments": arguments, "outcome": outcome }), true).await;
    }

    /// Write what Claude streams until the stream ends.
    pub async fn follow(&self, mut rx: UnboundedReceiver<StreamEvent>) {
        let mut open: HashMap<usize, Open> = HashMap::new();
        let mut last = Instant::now();
        while let Some(ev) = rx.recv().await {
            match ev {
                StreamEvent::BlockStart { index, kind, .. } => {
                    let kind = match kind.as_str() {
                        "thinking" => "thinking",
                        "text" => "text",
                        _ => continue,
                    };
                    open.insert(index, Open { id: None, kind, text: String::new(), written: 0 });
                }
                StreamEvent::Delta { index, text } => {
                    if let Some(o) = open.get_mut(&index) {
                        o.text.push_str(&text);
                    }
                }
                StreamEvent::BlockStop { index } => {
                    if let Some(mut o) = open.remove(&index) {
                        self.write(&mut o, true).await;
                    }
                }
                StreamEvent::Restart => {
                    for (_, mut o) in open.drain() {
                        o.text.push_str("\n\n(Interrupted; Claude is starting this step again.)");
                        self.write(&mut o, true).await;
                    }
                }
            }
            if last.elapsed() >= FLUSH_EVERY {
                last = Instant::now();
                for o in open.values_mut() {
                    if o.text.len() > o.written {
                        self.write(o, false).await;
                    }
                }
            }
        }
        // The request ended (or failed) with blocks still open.
        for (_, mut o) in open.drain() {
            self.write(&mut o, true).await;
        }
    }

    /// Insert or update a streaming entry. Empty blocks get no entry.
    async fn write(&self, o: &mut Open, done: bool) {
        let r = match o.id {
            None if o.text.trim().is_empty() => return,
            None => {
                let id = Uuid::new_v4();
                o.id = Some(id);
                self.insert_with_id(id, o.kind, &o.text, json!({}), done).await
            }
            Some(id) => sqlx::query("UPDATE axom.agent_log SET content = $2, done = $3, rev = nextval('axom.agent_log_rev'), updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&o.text)
                .bind(done)
                .execute(self.pool)
                .await
                .map(|_| ()),
        };
        match r {
            Ok(()) => o.written = o.text.len(),
            Err(e) => tracing::warn!(task = %self.task_id, error = %e, "agent log write failed"),
        }
    }

    async fn insert(&self, kind: &str, content: &str, detail: Value, done: bool) {
        if let Err(e) = self.insert_with_id(Uuid::new_v4(), kind, content, detail, done).await {
            tracing::warn!(task = %self.task_id, error = %e, "agent log write failed");
        }
    }

    async fn insert_with_id(&self, id: Uuid, kind: &str, content: &str, detail: Value, done: bool) -> sqlx::Result<()> {
        sqlx::query("INSERT INTO axom.agent_log (id, task_id, worker_id, worker_name, kind, content, detail, done) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(id)
            .bind(self.task_id)
            .bind(self.worker_id)
            .bind(&self.worker_name)
            .bind(kind)
            .bind(content)
            .bind(detail)
            .bind(done)
            .execute(self.pool)
            .await
            .map(|_| ())
    }
}
