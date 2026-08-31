CREATE TABLE tunnels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    base_url TEXT NOT NULL UNIQUE,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE mcp_agent_plugin_tokens (
    agent_id TEXT PRIMARY KEY REFERENCES mcp_agents(id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    token_plain TEXT NOT NULL,
    token_last4 TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_tunnels_updated_at ON tunnels(updated_at_ms DESC);
CREATE INDEX idx_plugin_tokens_hash ON mcp_agent_plugin_tokens(token_hash);

UPDATE schema_version SET version = 12 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '12' WHERE key = 'schema_version';
