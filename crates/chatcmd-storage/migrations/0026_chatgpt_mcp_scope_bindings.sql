-- Provider MCP session identities and browser URL identities are not interchangeable.
-- Remember the successfully resolved task, scoped to the authenticated agent/device.
CREATE TABLE chatgpt_mcp_scope_bindings (
    agent_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    scope_hash TEXT NOT NULL CHECK(length(scope_hash) BETWEEN 1 AND 256),
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK(generation >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(agent_id, device_id, scope_hash)
);
CREATE INDEX idx_chatgpt_mcp_scope_task ON chatgpt_mcp_scope_bindings(task_id);
CREATE INDEX idx_chatgpt_bridge_agent_turn ON chatgpt_bridge_requests(agent_id, turn_id);

UPDATE schema_version SET version = 26 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '26' WHERE key = 'schema_version';
