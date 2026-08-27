CREATE TABLE chatgpt_bridge_requests (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    turn_id TEXT NOT NULL,
    agent_id TEXT NOT NULL REFERENCES mcp_agents(id) ON DELETE RESTRICT,
    model TEXT NOT NULL,
    user_content TEXT NOT NULL,
    submitted_content TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued','running','stop_requested','completed','stopped','failed')),
    conversation_id TEXT,
    conversation_url TEXT,
    assistant_content TEXT,
    error_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER
) STRICT;
CREATE INDEX idx_chatgpt_bridge_requests_task ON chatgpt_bridge_requests(task_id, created_at_ms DESC);
CREATE INDEX idx_chatgpt_bridge_requests_status ON chatgpt_bridge_requests(status, updated_at_ms);

CREATE TABLE chatgpt_conversations (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL UNIQUE,
    conversation_url TEXT NOT NULL,
    model TEXT NOT NULL,
    active_request_id TEXT REFERENCES chatgpt_bridge_requests(id) ON DELETE SET NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

UPDATE schema_version SET version = 4 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '4' WHERE key = 'schema_version';
