-- Cross-chain payouts through NEAR Intents.
--
-- A worker with a verified NEAR address may name a destination on another
-- chain (chain + 1Click asset id + address) by signing it with that NEAR
-- key. Payroll then pays them there: the signer deposits wNEAR into a 1Click
-- quote and solvers deliver the destination asset.

CREATE TABLE axom.destination_challenges (
    id           UUID PRIMARY KEY,
    worker_id    UUID NOT NULL REFERENCES axom.workers (id),
    network      TEXT NOT NULL CHECK (network IN ('testnet','mainnet')),
    -- The worker's verified NEAR account and the key that must sign.
    near_account TEXT NOT NULL,
    public_key   TEXT NOT NULL,
    chain        TEXT NOT NULL,
    asset_id     TEXT NOT NULL,
    address      TEXT NOT NULL,
    message      TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    used_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.payout_destinations (
    id           UUID PRIMARY KEY,
    worker_id    UUID NOT NULL REFERENCES axom.workers (id),
    network      TEXT NOT NULL CHECK (network IN ('testnet','mainnet')),
    near_account TEXT NOT NULL,
    chain        TEXT NOT NULL,
    asset_id     TEXT NOT NULL,
    address      TEXT NOT NULL,
    challenge_id UUID NOT NULL REFERENCES axom.destination_challenges (id),
    signature    TEXT NOT NULL,
    verified_at  TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ
);
-- At most one active destination per worker per network.
CREATE UNIQUE INDEX payout_destinations_active_idx ON axom.payout_destinations (worker_id, network) WHERE revoked_at IS NULL;

-- 'refunded' = the intent deposit was returned to the treasury by NEAR Intents.
ALTER TABLE axom.payouts DROP CONSTRAINT payouts_status_check;
ALTER TABLE axom.payouts ADD CONSTRAINT payouts_status_check CHECK (status IN (
    'pending_approval','approved','sent','finalized','failed','rejected','expired','cap_exceeded','blocked','refunded'));

-- Destination fixed at creation (and covered by the signed authorization).
ALTER TABLE axom.payouts ADD COLUMN destination_id UUID REFERENCES axom.payout_destinations (id);
ALTER TABLE axom.payouts ADD COLUMN dest_chain TEXT;
ALTER TABLE axom.payouts ADD COLUMN dest_asset TEXT;
ALTER TABLE axom.payouts ADD COLUMN dest_address TEXT;
ALTER TABLE axom.payouts ADD CONSTRAINT payouts_destination_complete CHECK (
    (destination_id IS NULL AND dest_chain IS NULL AND dest_asset IS NULL AND dest_address IS NULL)
 OR (destination_id IS NOT NULL AND dest_chain IS NOT NULL AND dest_asset IS NOT NULL AND dest_address IS NOT NULL));
-- The 1Click quote the signer deposited into, kept whole with its signature
-- (1Click asks integrators to keep both to resolve disputes).
ALTER TABLE axom.payouts ADD COLUMN intent_deposit_address TEXT;
ALTER TABLE axom.payouts ADD COLUMN intent_quote JSONB;
ALTER TABLE axom.payouts ADD COLUMN intent_quote_signature TEXT;
-- Last 1Click execution status and destination-chain transactions.
ALTER TABLE axom.payouts ADD COLUMN intent_status TEXT;
ALTER TABLE axom.payouts ADD COLUMN intent_tx_hashes JSONB;
ALTER TABLE axom.payouts ADD COLUMN intent_checked_at TIMESTAMPTZ;
