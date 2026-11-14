-- Milestone 3: performance reviews, firing, owner notification delivery.

-- When a task reached done or failed (completed_at stays done-only).
ALTER TABLE axom.tasks ADD COLUMN finished_at TIMESTAMPTZ;
-- Objective quality check (tests passing / acceptance criteria met).
ALTER TABLE axom.tasks ADD COLUMN objective_check_passed BOOLEAN;
-- LLM-judged quality, 0..1, and the judge model (must differ from the worker's).
ALTER TABLE axom.tasks ADD COLUMN llm_quality_score DOUBLE PRECISION
    CHECK (llm_quality_score IS NULL OR (llm_quality_score >= 0 AND llm_quality_score <= 1));
ALTER TABLE axom.tasks ADD COLUMN llm_judge_model TEXT;
ALTER TABLE axom.tasks ADD CONSTRAINT llm_score_has_judge
    CHECK (llm_quality_score IS NULL OR llm_judge_model IS NOT NULL);
CREATE INDEX tasks_assignee_finished_idx ON axom.tasks (assignee, finished_at);

-- The model an AI worker runs on (used to keep LLM judges independent).
ALTER TABLE axom.workers ADD COLUMN model TEXT;
ALTER TABLE axom.workers ADD COLUMN fired_at TIMESTAMPTZ;

CREATE TABLE axom.reviews (
    id                     UUID PRIMARY KEY,
    worker_id              UUID NOT NULL REFERENCES axom.workers (id),
    period_start           TIMESTAMPTZ NOT NULL,
    period_end             TIMESTAMPTZ NOT NULL,
    trigger                TEXT NOT NULL CHECK (trigger IN ('task_count','period_end','final')),
    tasks_finished         INT NOT NULL,
    completed              INT NOT NULL,
    failed                 INT NOT NULL,
    completion_rate        DOUBLE PRECISION,
    on_time_rate           DOUBLE PRECISION,
    cost_per_completed_usd NUMERIC(18,6),
    objective_quality      DOUBLE PRECISION,
    llm_quality            DOUBLE PRECISION,
    objective_below        BOOLEAN NOT NULL,
    reasons                TEXT[] NOT NULL DEFAULT '{}',
    config_version         TEXT NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (period_end >= period_start)
);
CREATE INDEX reviews_worker_idx ON axom.reviews (worker_id, period_end);

-- Random sample of reviews for the owner to spot-check.
CREATE TABLE axom.spot_checks (
    id           UUID PRIMARY KEY,
    review_id    UUID NOT NULL UNIQUE REFERENCES axom.reviews (id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    owner_agrees BOOLEAN,
    answered_by  TEXT,
    note         TEXT,
    answered_at  TIMESTAMPTZ
);

ALTER TABLE axom.owner_notifications ADD COLUMN email_sent_at TIMESTAMPTZ;
ALTER TABLE axom.owner_notifications ADD COLUMN attempts INT NOT NULL DEFAULT 0;
ALTER TABLE axom.owner_notifications ADD COLUMN last_error TEXT;
ALTER TABLE axom.owner_notifications ADD COLUMN read_at TIMESTAMPTZ;
CREATE INDEX owner_notifications_pending_idx ON axom.owner_notifications (created_at) WHERE email_sent_at IS NULL;
