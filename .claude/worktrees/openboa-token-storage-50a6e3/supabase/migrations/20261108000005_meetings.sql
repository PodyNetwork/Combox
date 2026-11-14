-- Milestone 6: meetings (interviews), invites, consent, transcripts, scores.

CREATE TABLE axom.meetings (
    id                    UUID PRIMARY KEY,
    title                 TEXT NOT NULL,
    room                  TEXT NOT NULL UNIQUE,
    scheduled_at          TIMESTAMPTZ NOT NULL,
    invitees              TEXT[] NOT NULL,
    -- Recipients the owner added in the dashboard (send_summary allow-list).
    extra_recipients      TEXT[] NOT NULL DEFAULT '{}',
    candidate_worker_id   UUID REFERENCES axom.workers (id),
    status                TEXT NOT NULL DEFAULT 'scheduled' CHECK (status IN (
                              'scheduled','dispatched','completed','consent_declined','cancelled')),
    instructions          TEXT,
    questions             JSONB,
    rubric                JSONB,
    callback_token_sha256 TEXT,
    -- {granted, statement, at}: recorded before any question is asked.
    consent               JSONB,
    -- Untrusted: what participants said.
    transcript            JSONB,
    summary               TEXT,
    scores                JSONB,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    dispatched_at         TIMESTAMPTZ,
    completed_at          TIMESTAMPTZ,
    summary_sent_at       TIMESTAMPTZ
);

CREATE TABLE axom.meeting_invites (
    token_sha256 TEXT PRIMARY KEY,
    meeting_id   UUID NOT NULL REFERENCES axom.meetings (id),
    email        TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
