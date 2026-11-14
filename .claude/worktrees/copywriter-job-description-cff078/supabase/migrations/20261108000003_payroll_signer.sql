-- Milestone 4: payouts in NEAR, authorizations, approvals, address and tax gates.

-- 'blocked' = a payout gate (unverified address, missing tax form) failed.
ALTER TABLE axom.payouts DROP CONSTRAINT payouts_status_check;
ALTER TABLE axom.payouts ADD CONSTRAINT payouts_status_check CHECK (status IN (
    'pending_approval','approved','sent','finalized','failed','rejected','expired','cap_exceeded','blocked'));

ALTER TABLE axom.payouts ADD COLUMN network TEXT NOT NULL DEFAULT 'testnet' CHECK (network IN ('testnet','mainnet'));
ALTER TABLE axom.payouts ADD COLUMN amount_yocto NUMERIC(40,0) CHECK (amount_yocto IS NULL OR amount_yocto > 0);
ALTER TABLE axom.payouts ADD COLUMN usd_per_near NUMERIC(18,6);
ALTER TABLE axom.payouts ADD COLUMN price_fetched_at TIMESTAMPTZ;
ALTER TABLE axom.payouts ADD COLUMN period_start TIMESTAMPTZ;
ALTER TABLE axom.payouts ADD COLUMN period_end TIMESTAMPTZ;
ALTER TABLE axom.payouts ADD COLUMN tx_hash TEXT;
-- The exact signed transaction (base64). Retries after an unknown outcome
-- resubmit these bytes, so a transfer can never land twice.
ALTER TABLE axom.payouts ADD COLUMN signed_tx TEXT;
ALTER TABLE axom.payouts ADD COLUMN attempts INT NOT NULL DEFAULT 0;
ALTER TABLE axom.payouts ADD COLUMN next_attempt_at TIMESTAMPTZ;
ALTER TABLE axom.payouts ADD COLUMN last_error TEXT;
ALTER TABLE axom.payouts ADD COLUMN sent_at TIMESTAMPTZ;
ALTER TABLE axom.payouts ADD COLUMN finalized_at TIMESTAMPTZ;
-- One live payout per worker per pay period. Blocked payouts may be retried.
CREATE UNIQUE INDEX payouts_worker_period_idx ON axom.payouts (worker_id, period_start)
    WHERE worker_id IS NOT NULL AND period_start IS NOT NULL AND status <> 'blocked';

-- Hours for hourly workers.
ALTER TABLE axom.tasks ADD COLUMN hours_worked NUMERIC(10,2) CHECK (hours_worked IS NULL OR hours_worked >= 0);

-- Dashboard admins (Supabase Auth users).
CREATE TABLE axom.admins (
    user_id    UUID PRIMARY KEY,
    email      TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    active     BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.approvals (
    id           UUID PRIMARY KEY,
    payout_id    UUID NOT NULL REFERENCES axom.payouts (id),
    decision     TEXT NOT NULL CHECK (decision IN ('approve','reject')),
    admin_id     UUID NOT NULL REFERENCES axom.admins (user_id),
    admin_name   TEXT NOT NULL,
    session_id   TEXT NOT NULL,
    mfa_verified BOOLEAN NOT NULL CHECK (mfa_verified),
    reason       TEXT NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX approvals_one_per_payout_idx ON axom.approvals (payout_id);

-- Signed authorizations the signer verifies before every sign request.
CREATE TABLE axom.payout_authorizations (
    id         UUID PRIMARY KEY,
    payout_id  UUID NOT NULL REFERENCES axom.payouts (id),
    kind       TEXT NOT NULL CHECK (kind IN ('auto_attestation','human_approval')),
    payload    JSONB NOT NULL,
    signature  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX payout_authorizations_payout_idx ON axom.payout_authorizations (payout_id);

-- Worker NEAR addresses. A worker brings their own; verified by challenge signature.
CREATE TABLE axom.address_challenges (
    id          UUID PRIMARY KEY,
    worker_id   UUID NOT NULL REFERENCES axom.workers (id),
    account_id  TEXT NOT NULL,
    public_key  TEXT NOT NULL,
    message     TEXT NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.worker_addresses (
    id           UUID PRIMARY KEY,
    worker_id    UUID NOT NULL REFERENCES axom.workers (id),
    network      TEXT NOT NULL CHECK (network IN ('testnet','mainnet')),
    account_id   TEXT NOT NULL,
    public_key   TEXT NOT NULL,
    challenge_id UUID NOT NULL REFERENCES axom.address_challenges (id),
    verified_at  TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ,
    UNIQUE (worker_id, network, account_id)
);

CREATE TABLE axom.tax_forms (
    id                     UUID PRIMARY KEY,
    worker_id              UUID NOT NULL REFERENCES axom.workers (id),
    kind                   TEXT NOT NULL CHECK (kind IN ('w9','w8ben')),
    docuseal_submission_id TEXT,
    received_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (worker_id, kind)
);
