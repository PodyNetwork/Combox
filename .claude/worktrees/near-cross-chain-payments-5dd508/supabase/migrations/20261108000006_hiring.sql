-- Milestone 8: Upwork job postings, candidates, and contracts.

CREATE TABLE axom.job_postings (
    id                 UUID PRIMARY KEY,
    hiring_request_id  UUID NOT NULL UNIQUE REFERENCES axom.hiring_requests (id),
    external_job_id    TEXT NOT NULL,
    spec               JSONB NOT NULL,
    contract_template  TEXT NOT NULL,
    status             TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','filled','closed')),
    policy_version     TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.candidates (
    id                     UUID PRIMARY KEY,
    job_posting_id         UUID NOT NULL REFERENCES axom.job_postings (id),
    external_applicant_id  TEXT NOT NULL,
    name                   TEXT NOT NULL,
    email                  TEXT,
    skills                 TEXT[] NOT NULL DEFAULT '{}',
    -- Untrusted: the applicant's own message, stored as data only.
    message                TEXT NOT NULL DEFAULT '',
    shortlist_score        DOUBLE PRECISION NOT NULL DEFAULT 0,
    status                 TEXT NOT NULL DEFAULT 'applied' CHECK (status IN (
                               'applied','shortlisted','invited','interviewed','contract_sent','hired','rejected','declined')),
    meeting_id             UUID REFERENCES axom.meetings (id),
    rubric                 JSONB,
    rubric_score           DOUBLE PRECISION,
    contract_submission_id TEXT,
    worker_id              UUID REFERENCES axom.workers (id),
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_posting_id, external_applicant_id)
);
