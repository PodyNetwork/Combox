-- Milestone 5: task intake queue (internal, in axom-api) and freelancer deliverables.

CREATE TABLE axom.intake_items (
    id            UUID PRIMARY KEY,
    task_id       UUID NOT NULL UNIQUE REFERENCES axom.tasks (id),
    brief         TEXT NOT NULL,
    required_skills TEXT[] NOT NULL DEFAULT '{}',
    assignee      UUID REFERENCES axom.workers (id),
    status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','submitted','accepted','returned')),
    -- Deliverable as submitted by the freelancer: untrusted data.
    deliverable   JSONB,
    submitted_at  TIMESTAMPTZ,
    review_note   TEXT,
    reviewed_at   TIMESTAMPTZ,
    reviewed_by   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX intake_items_assignee_idx ON axom.intake_items (assignee, status, created_at);

-- Per-freelancer access tokens for the intake API (only the hash is stored).
CREATE TABLE axom.worker_tokens (
    token_sha256 TEXT PRIMARY KEY,
    worker_id    UUID NOT NULL REFERENCES axom.workers (id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);
