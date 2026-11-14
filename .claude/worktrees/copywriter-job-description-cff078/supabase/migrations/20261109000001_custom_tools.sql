-- Tools admins add themselves: MCP servers (Axom discovers their tools) and
-- HTTP webhooks. AI workers can use a tool only when an MFA admin puts it on
-- their allowlist, and every call passes the policy engine.

CREATE TABLE axom.tool_servers (
    id             UUID PRIMARY KEY,
    -- Prefix of the tool names in allowlists: "github" -> "github.create_issue".
    name           TEXT NOT NULL UNIQUE CHECK (name ~ '^[a-z][a-z0-9_-]{1,30}$'),
    kind           TEXT NOT NULL CHECK (kind IN ('mcp','webhook')),
    url            TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    -- Header carrying the token, e.g. "Authorization" (value "Bearer ...").
    auth_header    TEXT,
    -- The header value, encrypted (AES-256-GCM, AXOM_SECRETS_KEY), bound to this row.
    secret_nonce   BYTEA,
    secret_ct      BYTEA,
    secret_hint    TEXT,
    enabled        BOOLEAN NOT NULL DEFAULT true,
    last_synced_at TIMESTAMPTZ,
    last_error     TEXT,
    created_by     TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE axom.custom_tools (
    id           UUID PRIMARY KEY,
    server_id    UUID NOT NULL REFERENCES axom.tool_servers (id) ON DELETE CASCADE,
    -- The name used in worker allowlists and task requirements.
    tool_id      TEXT NOT NULL UNIQUE,
    -- The MCP server's own name for the tool (NULL for a webhook).
    remote_name  TEXT,
    description  TEXT NOT NULL DEFAULT '',
    input_schema JSONB NOT NULL DEFAULT '{}',
    enabled      BOOLEAN NOT NULL DEFAULT true,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
