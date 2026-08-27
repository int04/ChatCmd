CREATE TABLE subagent_runs (
    id TEXT PRIMARY KEY NOT NULL,
    parent_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    parent_turn_id TEXT NOT NULL,
    child_task_id TEXT UNIQUE REFERENCES tasks(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    request TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','running','completed','failed','stopped','interrupted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER
) STRICT;
CREATE INDEX idx_subagent_runs_parent ON subagent_runs(parent_task_id, parent_turn_id, created_at_ms);
CREATE INDEX idx_subagent_runs_child ON subagent_runs(child_task_id);
CREATE INDEX idx_subagent_runs_status ON subagent_runs(status, updated_at_ms DESC);

UPDATE schema_version SET version = 3 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '3' WHERE key = 'schema_version';
