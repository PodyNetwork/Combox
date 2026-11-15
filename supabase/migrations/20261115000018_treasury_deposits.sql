-- NEAR others sent to the treasury (deposits), read from the chain, so the
-- wallet's history pages and filters in SQL with the payouts and Rhea moves
-- already recorded here. Every inbound transaction the sync looked at is
-- kept, with 0 when it moved no NEAR to the treasury, so it isn't fetched
-- again; the history shows only those with an amount.
CREATE TABLE axom.treasury_deposits (
    tx_hash      TEXT PRIMARY KEY,
    network      TEXT NOT NULL CHECK (network IN ('testnet','mainnet')),
    -- The treasury account it reached.
    account      TEXT NOT NULL,
    sender       TEXT NOT NULL,
    amount_yocto NUMERIC(40,0) NOT NULL CHECK (amount_yocto >= 0),
    status       TEXT NOT NULL CHECK (status IN ('succeeded','failed')),
    block_height BIGINT NOT NULL,
    -- When the transfer executed (block time).
    created_at   TIMESTAMPTZ NOT NULL,
    synced_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX treasury_deposits_history_idx ON axom.treasury_deposits (network, account, created_at DESC);
