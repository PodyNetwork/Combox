-- AI workers can ask the people they work for a question and wait for the
-- answer. The task is blocked with `result.waiting_for = 'reply'` meanwhile;
-- an admin's reply sends it back to the worker, which carries on from where
-- it stopped (its conversation with Claude is kept here until then).

CREATE TABLE axom.task_threads (
    task_id     UUID PRIMARY KEY REFERENCES axom.tasks (id) ON DELETE CASCADE,
    -- The Messages API conversation up to the reply that asked.
    messages    JSONB NOT NULL,
    -- Results of other tool calls made in that same reply.
    results     JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- Tool calls made so far (for the final report).
    calls       JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- The tool call that asked; the reply answers it.
    ask_id      TEXT NOT NULL,
    questions   TEXT NOT NULL,
    reply       TEXT,
    replied_by  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The agent log shows the questions (open until answered) and the replies.
ALTER TABLE axom.agent_log DROP CONSTRAINT agent_log_kind_check;
ALTER TABLE axom.agent_log ADD CONSTRAINT agent_log_kind_check
    CHECK (kind IN ('status','thinking','text','tool','question','reply'));
