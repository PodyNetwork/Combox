-- Treasury yield on Rhea lending (formerly Burrow): idle treasury NEAR is
-- supplied while payroll doesn't need it and withdrawn when it does.

-- Every move the signer makes: register, supply, withdraw, unwrap. All keep
-- value inside the treasury's own accounts.
CREATE TABLE axom.treasury_moves (
    id           UUID PRIMARY KEY,
    network      TEXT NOT NULL CHECK (network IN ('testnet','mainnet')),
    kind         TEXT NOT NULL CHECK (kind IN ('register','supply','withdraw','unwrap')),
    amount_yocto NUMERIC(40,0) NOT NULL CHECK (amount_yocto > 0),
    contract     TEXT NOT NULL,
    reason       TEXT NOT NULL,
    plan         JSONB NOT NULL,
    policy_version TEXT NOT NULL,
    -- Same idempotency rule as payouts: the exact signed bytes are kept and
    -- resubmitted on an unknown outcome, never re-signed.
    tx_hash      TEXT NOT NULL,
    signed_tx    TEXT NOT NULL,
    -- Shares the treasury key's nonces with payouts (axom.payouts.tx_nonce).
    tx_nonce     BIGINT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('submitted','succeeded','failed')),
    error        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- At most one move in flight at a time.
CREATE UNIQUE INDEX treasury_moves_one_in_flight_idx ON axom.treasury_moves (network) WHERE status = 'submitted';
CREATE INDEX treasury_moves_created_idx ON axom.treasury_moves (created_at DESC);

-- The planner's latest view (position, floor, target, decision), for the dashboard.
CREATE TABLE axom.treasury_status (
    id         SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    network    TEXT NOT NULL,
    account    TEXT NOT NULL,
    plan       JSONB NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL
);
