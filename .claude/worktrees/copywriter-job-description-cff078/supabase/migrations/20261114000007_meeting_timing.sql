-- Timed interviews: join links expire shortly after the start time, meetings
-- nobody attended are marked expired, and reminders are sent before the start.

ALTER TABLE axom.meetings DROP CONSTRAINT meetings_status_check;
ALTER TABLE axom.meetings ADD CONSTRAINT meetings_status_check CHECK (status IN (
    'scheduled','dispatched','completed','consent_declined','cancelled','expired'));
ALTER TABLE axom.meetings ADD COLUMN duration_minutes INT NOT NULL DEFAULT 10 CHECK (duration_minutes BETWEEN 1 AND 120);
ALTER TABLE axom.meetings ADD COLUMN expired_at TIMESTAMPTZ;

-- Links an admin opens from the dashboard: they work before the start too.
ALTER TABLE axom.meeting_invites ADD COLUMN host BOOLEAN NOT NULL DEFAULT false;

-- Existing invitee links (not admin links): valid until 10 minutes after the start, as for new meetings.
UPDATE axom.meeting_invites i SET expires_at = LEAST(i.expires_at, m.scheduled_at + interval '10 minutes')
FROM axom.meetings m WHERE m.id = i.meeting_id AND i.email = ANY(m.invitees);

-- One row per reminder slot per meeting ('24h', '1h', '10m'). `sent` is false
-- when the slot was skipped (the meeting was booked after it had passed) or
-- no message went out. No addresses are kept here: only how many were reached.
CREATE TABLE axom.meeting_reminders (
    meeting_id  UUID NOT NULL REFERENCES axom.meetings (id),
    slot        TEXT NOT NULL,
    sent        BOOLEAN NOT NULL,
    sent_count  INT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (meeting_id, slot)
);

CREATE INDEX meetings_open_by_time ON axom.meetings (scheduled_at) WHERE status IN ('scheduled','dispatched');
