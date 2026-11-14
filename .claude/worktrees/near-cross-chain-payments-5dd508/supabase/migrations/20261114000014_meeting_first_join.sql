-- Interview links last 24 hours after the start, but once the first invitee
-- joins, invitee links stop working when the interview's time limit passes.
ALTER TABLE axom.meetings ADD COLUMN first_joined_at TIMESTAMPTZ;

-- Interviews now last 15 minutes (existing meetings keep their own limit).
ALTER TABLE axom.meetings ALTER COLUMN duration_minutes SET DEFAULT 15;
