-- Subscription emails already sent, so each is sent once per period:
-- a few days before a trial or paid period ends, and when it has ended.
CREATE TABLE platform.reminders (
    tenant_id   UUID NOT NULL REFERENCES platform.tenants (id),
    kind        TEXT NOT NULL CHECK (kind IN ('trial_ending','subscription_ending','expired')),
    period_end  TIMESTAMPTZ NOT NULL,
    sent_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, kind, period_end)
);
