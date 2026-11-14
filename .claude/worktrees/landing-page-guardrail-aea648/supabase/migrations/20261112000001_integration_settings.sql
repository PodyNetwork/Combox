-- Single-company deployments: integration settings and API keys set from the
-- dashboard, encrypted with AXOM_SECRETS_KEY. A value here overrides .env.
-- (The multi-tenant platform keeps these in platform.secrets instead.)
CREATE TABLE axom.integration_settings (
    name        TEXT PRIMARY KEY,
    nonce       BYTEA NOT NULL,
    ciphertext  BYTEA NOT NULL,
    -- The value for plain settings; only the last four characters for secrets.
    hint        TEXT NOT NULL,
    updated_by  UUID NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
