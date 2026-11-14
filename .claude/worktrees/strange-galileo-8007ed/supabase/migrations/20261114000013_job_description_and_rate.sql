-- A worker's hiring request can carry the job description to post and the
-- per-task rate the owner gave (bounded by the payout approval threshold
-- when the request is opened). Both NULL for requests opened by routing:
-- the posting then uses the planned role's rate and no description.

ALTER TABLE axom.hiring_requests
    ADD COLUMN description TEXT,
    ADD COLUMN rate_usd NUMERIC(12,2) CHECK (rate_usd > 0);

-- Shown on the public board as plain text.
ALTER TABLE axom.board_jobs ADD COLUMN description TEXT NOT NULL DEFAULT '';
