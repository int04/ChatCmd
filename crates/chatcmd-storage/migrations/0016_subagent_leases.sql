ALTER TABLE subagent_runs RENAME TO subagent_runs_without_leases;

CREATE TABLE subagent_runs (
    id TEXT PRIMARY KEY NOT NULL,
    parent_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    parent_turn_id TEXT NOT NULL,
    child_task_id TEXT UNIQUE REFERENCES tasks(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    request TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','running','completed','failed','stopped','timedOut','interrupted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    fallback_state TEXT NOT NULL DEFAULT 'none' CHECK (fallback_state IN ('none','requested','started','claimed','exhausted')),
    fallback_attempts INTEGER NOT NULL DEFAULT 0,
    fallback_error TEXT,
    fallback_conversation_id TEXT,
    fallback_conversation_url TEXT,
    worker_id TEXT,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    lease_acquired_at_ms INTEGER,
    lease_expires_at_ms INTEGER,
    last_heartbeat_at_ms INTEGER,
    max_runtime_ms INTEGER NOT NULL DEFAULT 1800000 CHECK (max_runtime_ms > 0),
    started_at_ms INTEGER,
    terminal_reason TEXT
) STRICT;

INSERT INTO subagent_runs (
    id,parent_task_id,parent_turn_id,child_task_id,name,request,status,
    created_at_ms,updated_at_ms,completed_at_ms,fallback_state,fallback_attempts,
    fallback_error,fallback_conversation_id,fallback_conversation_url
)
SELECT id,parent_task_id,parent_turn_id,child_task_id,name,request,status,
       created_at_ms,updated_at_ms,completed_at_ms,fallback_state,fallback_attempts,
       fallback_error,fallback_conversation_id,fallback_conversation_url
FROM subagent_runs_without_leases;

DROP TABLE subagent_runs_without_leases;

CREATE INDEX idx_subagent_runs_parent ON subagent_runs(parent_task_id, parent_turn_id, created_at_ms);
CREATE INDEX idx_subagent_runs_child ON subagent_runs(child_task_id);
CREATE INDEX idx_subagent_runs_status_lease ON subagent_runs(status, lease_expires_at_ms, started_at_ms);
CREATE INDEX idx_subagent_runs_parent_status ON subagent_runs(parent_task_id, parent_turn_id, status);
CREATE INDEX idx_subagent_runs_worker_attempt ON subagent_runs(worker_id, attempt);

UPDATE schema_version SET version = 16 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '16' WHERE key = 'schema_version';
