-- Multi-tenant control plane (the "platform" database). Each company's HR
-- data lives in its own database, created from supabase/migrations; this
-- database only knows companies, their members, encrypted integration keys,
-- and subscription invoices.

CREATE SCHEMA IF NOT EXISTS platform;

CREATE TABLE platform.tenants (
    id               UUID PRIMARY KEY,
    slug             TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]([a-z0-9-]{1,38}[a-z0-9])$'),
    name             TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    db_name          TEXT NOT NULL UNIQUE,
    status           TEXT NOT NULL DEFAULT 'provisioning' CHECK (status IN ('provisioning','active','suspended')),
    trial_ends_at    TIMESTAMPTZ NOT NULL,
    paid_until       TIMESTAMPTZ,
    -- OpenBao transit key holding this company's NEAR treasury (never exported).
    treasury_key     TEXT,
    treasury_account TEXT,
    owner_email      TEXT NOT NULL,
    created_by       UUID NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE platform.members (
    tenant_id   UUID NOT NULL REFERENCES platform.tenants (id),
    user_id     UUID NOT NULL,
    email       TEXT NOT NULL,
    name        TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner','admin')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);
CREATE INDEX members_user ON platform.members (user_id);

-- Integration keys, encrypted with AES-256-GCM under AXOM_SECRETS_KEY; the
-- tenant id and key name are bound in as associated data, so a ciphertext
-- can't be moved to another company or key.
CREATE TABLE platform.secrets (
    tenant_id   UUID NOT NULL REFERENCES platform.tenants (id),
    name        TEXT NOT NULL CHECK (name ~ '^[A-Z][A-Z0-9_]{1,63}$'),
    nonce       BYTEA NOT NULL,
    ciphertext  BYTEA NOT NULL,
    hint        TEXT NOT NULL,
    updated_by  UUID NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, name)
);

CREATE TABLE platform.invoices (
    id            UUID PRIMARY KEY,
    tenant_id     UUID NOT NULL REFERENCES platform.tenants (id),
    months        INT NOT NULL CHECK (months BETWEEN 1 AND 12),
    amount_usd    NUMERIC(12,2) NOT NULL,
    usd_per_near  NUMERIC(20,6) NOT NULL,
    amount_yocto  NUMERIC(40,0) NOT NULL,
    pay_to        TEXT NOT NULL,
    network       TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','paid','expired')),
    tx_hash       TEXT UNIQUE,
    payer         TEXT,
    created_by    UUID NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    paid_at       TIMESTAMPTZ
);
CREATE INDEX invoices_tenant ON platform.invoices (tenant_id, created_at DESC);

-- Append-only record of platform actions (signups, key changes, payments).
CREATE TABLE platform.audit_log (
    seq         BIGSERIAL PRIMARY KEY,
    tenant_id   UUID,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    details     JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE FUNCTION platform.audit_log_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'platform.audit_log is append-only';
END;
$$;
CREATE TRIGGER audit_log_no_update BEFORE UPDATE OR DELETE ON platform.audit_log
    FOR EACH ROW EXECUTE FUNCTION platform.audit_log_immutable();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON platform.audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION platform.audit_log_immutable();

-- Invitations by email: accepted when a signed-in, MFA-verified user with
-- that (Supabase-confirmed) email opens the dashboard.
CREATE TABLE platform.invites (
    id          UUID PRIMARY KEY,
    tenant_id   UUID NOT NULL REFERENCES platform.tenants (id),
    email       TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'admin' CHECK (role IN ('admin')),
    invited_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);
CREATE UNIQUE INDEX invites_open ON platform.invites (tenant_id, lower(email)) WHERE accepted_at IS NULL AND revoked_at IS NULL;
