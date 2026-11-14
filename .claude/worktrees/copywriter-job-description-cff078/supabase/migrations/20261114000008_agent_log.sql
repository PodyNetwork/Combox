-- The agent log: what AI workers are doing and thinking, written while they
-- work so the dashboard can show it live. One row per entry: a thinking
-- summary or reply text (updated as Claude streams it), a tool call, or a
-- status change. Not the audit record (that stays in axom.audit_log); rows
-- older than two weeks are deleted.
--
-- `rev` comes from one sequence and is bumped on every write, so a reader
-- asks for "everything with rev > the last one I saw" and gets new rows and
-- rows that changed since.

CREATE SEQUENCE axom.agent_log_rev;

CREATE TABLE axom.agent_log (
    id          UUID PRIMARY KEY,
    rev         BIGINT NOT NULL DEFAULT nextval('axom.agent_log_rev'),
    task_id     UUID NOT NULL REFERENCES axom.tasks (id) ON DELETE CASCADE,
    worker_id   UUID REFERENCES axom.workers (id) ON DELETE SET NULL,
    worker_name TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('status','thinking','text','tool')),
    content     TEXT NOT NULL DEFAULT '',
    detail      JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- false while Claude is still streaming this entry.
    done        BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX agent_log_rev_idx ON axom.agent_log (rev);
CREATE INDEX agent_log_task_idx ON axom.agent_log (task_id, created_at);
CREATE INDEX agent_log_created_idx ON axom.agent_log (created_at);
