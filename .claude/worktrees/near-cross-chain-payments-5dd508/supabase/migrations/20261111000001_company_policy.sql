-- A company's own payout and privacy rules. They can only be stricter than
-- the platform's config/policy.toml: a lower approval threshold (more
-- payouts wait for a human) and a shorter candidate data retention.
CREATE TABLE axom.company_policy (
    id                       SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    approval_threshold_usd   NUMERIC(18,2) CHECK (approval_threshold_usd >= 0),
    candidate_retention_days INT CHECK (candidate_retention_days >= 1),
    updated_by               TEXT NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);
