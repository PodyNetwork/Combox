-- Retrying failed tasks. `retries` counts automatic retries since the task
-- was last queued by hand (an admin's retry starts the count again).
-- Whether a failure is worth retrying automatically is recorded in the
-- task's result (`result.retryable`) when it fails.

ALTER TABLE axom.tasks ADD COLUMN retries INT NOT NULL DEFAULT 0;

CREATE INDEX tasks_failed_idx ON axom.tasks (finished_at) WHERE status = 'failed';
