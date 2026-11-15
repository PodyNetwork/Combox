-- Every task has a start time. Work starts on it as soon as it is due:
-- right away for a task that starts now (the default), or at the time set
-- for a scheduled one. Nothing is routed or run before its start time.
ALTER TABLE axom.tasks ADD COLUMN start_at TIMESTAMPTZ NOT NULL DEFAULT now();
-- Existing tasks were ready when they were created.
UPDATE axom.tasks SET start_at = created_at;

CREATE INDEX tasks_queued_by_start ON axom.tasks (start_at) WHERE status = 'queued';

-- Wake the company's task loop the moment there is work to pick up: a new
-- task, or one that goes (back) to queued. Delivered on commit.
CREATE FUNCTION axom.notify_task_ready() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status = 'queued' AND (TG_OP = 'INSERT' OR OLD.status IS DISTINCT FROM NEW.status) THEN
        PERFORM pg_notify('axom_tasks', NEW.id::text);
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER tasks_notify_ready AFTER INSERT OR UPDATE OF status ON axom.tasks
    FOR EACH ROW EXECUTE FUNCTION axom.notify_task_ready();
