-- Axom's own jobs board: an alternative to Upwork for hiring freelancers
-- who are paid directly in NEAR (Upwork forbids paying its freelancers
-- off-platform without a conversion fee).

CREATE TABLE axom.board_jobs (
    id          UUID PRIMARY KEY,
    title       TEXT NOT NULL,
    skills      TEXT[] NOT NULL DEFAULT '{}',
    rate_usd    NUMERIC(12,2) NOT NULL,
    kpis        TEXT[] NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at   TIMESTAMPTZ
);

CREATE TABLE axom.board_applications (
    id           UUID PRIMARY KEY,
    job_id       UUID NOT NULL REFERENCES axom.board_jobs (id),
    name         TEXT NOT NULL,
    email        TEXT NOT NULL,
    skills       TEXT[] NOT NULL DEFAULT '{}',
    -- Untrusted: the applicant's own message, stored as data only.
    message      TEXT NOT NULL DEFAULT '',
    portfolio_url TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_id, email)
);

CREATE INDEX board_applications_job ON axom.board_applications (job_id, created_at);

-- Where each posting was published. Existing rows came from the Upwork path.
ALTER TABLE axom.job_postings
    ADD COLUMN marketplace TEXT NOT NULL DEFAULT 'upwork' CHECK (marketplace IN ('upwork','axom_board'));

