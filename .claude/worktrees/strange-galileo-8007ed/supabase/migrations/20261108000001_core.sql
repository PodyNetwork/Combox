-- Milestone 1: core domain, audit log, payouts, monthly cap ledger.
-- Milestone 2: plans, org chart, tasks, hiring requests.
-- All money columns are NUMERIC(18,6) in USD unless the name says otherwise.

CREATE SCHEMA IF NOT EXISTS axom;

-- ---------------------------------------------------------------------------
-- Audit log (append-only)
-- ---------------------------------------------------------------------------
CREATE TABLE axom.audit_log (
    id                UUID PRIMARY KEY,
    seq               BIGSERIAL UNIQUE NOT NULL,
    actor             TEXT NOT NULL,
    action            TEXT NOT NULL,
    inputs            JSONB NOT NULL DEFAULT '{}'::jsonb,
    reasoning_summary TEXT NOT NULL DEFAULT '',
    outcome           TEXT NOT NULL,
    policy_version    TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_action_idx ON axom.audit_log (action, created_at);

CREATE FUNCTION axom.audit_log_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only';
END;
$$;

CREATE TRIGGER audit_log_no_update BEFORE UPDATE OR DELETE ON axom.audit_log
    FOR EACH ROW EXECUTE FUNCTION axom.audit_log_immutable();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON axom.audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION axom.audit_log_immutable();

-- ---------------------------------------------------------------------------
-- Owner notifications (record only; queue + delivery arrive in milestone 3)
-- ---------------------------------------------------------------------------
CREATE TABLE axom.owner_notifications (
    id           UUID PRIMARY KEY,
    kind         TEXT NOT NULL,
    subject      TEXT NOT NULL,
    body         JSONB NOT NULL,
    audit_id     UUID REFERENCES axom.audit_log (id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- System controls: kill switch and dry-run live here so a human can flip
-- them from the dashboard. Agents have no write path to this table.
-- ---------------------------------------------------------------------------
CREATE TABLE axom.system_controls (
    id           SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    kill_switch  BOOLEAN NOT NULL DEFAULT false,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by   TEXT NOT NULL DEFAULT 'migration'
);
INSERT INTO axom.system_controls (id) VALUES (1);

-- ---------------------------------------------------------------------------
-- Monthly cap
-- ---------------------------------------------------------------------------
-- Single-row table holding the shared cap. Seeded on first boot from
-- MONTHLY_PAYOUT_CAP_USD; afterwards changed only by an MFA-verified admin.
CREATE TABLE axom.cap_settings (
    id         SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    cap_usd    NUMERIC(18,6) NOT NULL CHECK (cap_usd >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by TEXT NOT NULL
);

-- One row per UTC calendar month. Reservations lock this row (FOR UPDATE)
-- so concurrent payouts serialize on the month.
CREATE TABLE axom.cap_months (
    month      DATE PRIMARY KEY CHECK (extract(day FROM month) = 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.payouts (
    id              UUID PRIMARY KEY,
    worker_id       UUID,
    recipient       TEXT NOT NULL,
    amount_usd      NUMERIC(18,6) NOT NULL CHECK (amount_usd >= 0),
    fee_usd         NUMERIC(18,6) NOT NULL DEFAULT 0 CHECK (fee_usd >= 0),
    status          TEXT NOT NULL CHECK (status IN (
                        'pending_approval','approved','sent','finalized',
                        'failed','rejected','expired','cap_exceeded')),
    status_reason   TEXT NOT NULL DEFAULT '',
    policy_version  TEXT NOT NULL,
    reservation_id  UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payouts_recipient_idx ON axom.payouts (recipient, created_at);

CREATE TABLE axom.cap_ledger (
    id           UUID PRIMARY KEY,
    payout_id    UUID NOT NULL REFERENCES axom.payouts (id),
    month        DATE NOT NULL REFERENCES axom.cap_months (month),
    kind         TEXT NOT NULL CHECK (kind IN ('reservation','commit','release')),
    amount_usd   NUMERIC(18,6) NOT NULL CHECK (amount_usd >= 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- At most one entry of each kind per payout.
CREATE UNIQUE INDEX cap_ledger_payout_kind_idx ON axom.cap_ledger (payout_id, kind);
CREATE INDEX cap_ledger_month_idx ON axom.cap_ledger (month);

CREATE TABLE axom.cap_changes (
    id          UUID PRIMARY KEY,
    old_cap_usd NUMERIC(18,6),
    new_cap_usd NUMERIC(18,6) NOT NULL,
    admin       TEXT NOT NULL,
    audit_id    UUID NOT NULL REFERENCES axom.audit_log (id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Plans and org chart (milestone 2)
-- ---------------------------------------------------------------------------
CREATE TABLE axom.plans (
    id             UUID PRIMARY KEY,
    content_hash   TEXT NOT NULL UNIQUE,
    goal           TEXT NOT NULL,
    budget_usd     NUMERIC(18,6) NOT NULL CHECK (budget_usd >= 0),
    deadline       DATE NOT NULL,
    objectives     JSONB NOT NULL,
    source         JSONB NOT NULL,
    proposal       JSONB NOT NULL,
    policy_version TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.departments (
    id      UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES axom.plans (id),
    name    TEXT NOT NULL,
    UNIQUE (plan_id, name)
);

CREATE TABLE axom.teams (
    id            UUID PRIMARY KEY,
    department_id UUID NOT NULL REFERENCES axom.departments (id),
    name          TEXT NOT NULL,
    UNIQUE (department_id, name)
);

CREATE TABLE axom.roles (
    id             UUID PRIMARY KEY,
    team_id        UUID NOT NULL REFERENCES axom.teams (id),
    title          TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('ai_agent','human_freelancer')),
    skills         TEXT[] NOT NULL DEFAULT '{}',
    tool_allowlist TEXT[] NOT NULL DEFAULT '{}',
    pay_rate_usd   NUMERIC(18,6) NOT NULL CHECK (pay_rate_usd >= 0),
    pay_unit       TEXT NOT NULL CHECK (pay_unit IN ('task','hour')),
    kpis           JSONB NOT NULL DEFAULT '[]'::jsonb,
    headcount      INT NOT NULL CHECK (headcount >= 0),
    UNIQUE (team_id, title)
);

CREATE TABLE axom.workers (
    id             UUID PRIMARY KEY,
    team_id        UUID REFERENCES axom.teams (id),
    role_id        UUID REFERENCES axom.roles (id),
    name           TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('ai_agent','human_freelancer')),
    role           TEXT NOT NULL,
    status         TEXT NOT NULL CHECK (status IN (
                        'candidate','active','on_probation','suspended','fired')),
    skills         TEXT[] NOT NULL DEFAULT '{}',
    tool_allowlist TEXT[] NOT NULL DEFAULT '{}',
    pay_rate_usd   NUMERIC(18,6) NOT NULL CHECK (pay_rate_usd >= 0),
    pay_unit       TEXT NOT NULL CHECK (pay_unit IN ('task','hour')),
    wallet_address TEXT,
    kpis           JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Human workers never hold tool allowlists (they never execute tools).
ALTER TABLE axom.workers ADD CONSTRAINT humans_have_no_tools
    CHECK (kind <> 'human_freelancer' OR cardinality(tool_allowlist) = 0);

ALTER TABLE axom.payouts ADD CONSTRAINT payouts_worker_fk
    FOREIGN KEY (worker_id) REFERENCES axom.workers (id);

CREATE TABLE axom.tasks (
    id                UUID PRIMARY KEY,
    plan_id           UUID REFERENCES axom.plans (id),
    department_id     UUID REFERENCES axom.departments (id),
    seq               INT NOT NULL DEFAULT 0,
    objective         TEXT NOT NULL,
    required_tools    TEXT[] NOT NULL DEFAULT '{}',
    required_skills   TEXT[] NOT NULL DEFAULT '{}',
    priority          INT NOT NULL DEFAULT 3 CHECK (priority BETWEEN 1 AND 5),
    deadline          DATE,
    assignee          UUID REFERENCES axom.workers (id),
    status            TEXT NOT NULL CHECK (status IN (
                        'queued','assigned','in_progress','blocked','done','failed')),
    result            JSONB,
    cost_usd          NUMERIC(18,6) NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at      TIMESTAMPTZ
);
CREATE UNIQUE INDEX tasks_plan_seq_idx ON axom.tasks (plan_id, seq) WHERE plan_id IS NOT NULL;
CREATE INDEX tasks_assignee_idx ON axom.tasks (assignee, status);

CREATE TABLE axom.hiring_requests (
    id              UUID PRIMARY KEY,
    task_id         UUID REFERENCES axom.tasks (id),
    kind            TEXT NOT NULL CHECK (kind IN ('ai_agent','human_freelancer')),
    required_tools  TEXT[] NOT NULL DEFAULT '{}',
    required_skills TEXT[] NOT NULL DEFAULT '{}',
    reason          TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','filled','cancelled')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- One open hiring request per task.
CREATE UNIQUE INDEX hiring_requests_open_task_idx ON axom.hiring_requests (task_id)
    WHERE status = 'open' AND task_id IS NOT NULL;
