CREATE TABLE approval_grants (
    id TEXT PRIMARY KEY NOT NULL,
    owner_agent_id TEXT NOT NULL REFERENCES mcp_agents(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    turn_id TEXT,
    child_attempt INTEGER CHECK (child_attempt IS NULL OR child_attempt > 0),
    allowed_tools_json TEXT NOT NULL CHECK (json_valid(allowed_tools_json)),
    path_scopes_json TEXT NOT NULL CHECK (json_valid(path_scopes_json)),
    option_constraints_json TEXT NOT NULL CHECK (json_valid(option_constraints_json)),
    max_calls INTEGER NOT NULL CHECK (max_calls > 0),
    used_calls INTEGER NOT NULL DEFAULT 0 CHECK (used_calls >= 0),
    max_files_scanned INTEGER CHECK (max_files_scanned IS NULL OR max_files_scanned >= 0),
    used_files_scanned INTEGER NOT NULL DEFAULT 0 CHECK (used_files_scanned >= 0),
    max_bytes_read INTEGER CHECK (max_bytes_read IS NULL OR max_bytes_read >= 0),
    used_bytes_read INTEGER NOT NULL DEFAULT 0 CHECK (used_bytes_read >= 0),
    max_bytes_written INTEGER CHECK (max_bytes_written IS NULL OR max_bytes_written >= 0),
    used_bytes_written INTEGER NOT NULL DEFAULT 0 CHECK (used_bytes_written >= 0),
    expires_at_ms INTEGER NOT NULL,
    inherited_from TEXT REFERENCES approval_grants(id) ON DELETE CASCADE,
    catalog_hash TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active','revoked','expired','exhausted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_approval_grants_match ON approval_grants(task_id, owner_agent_id, state, expires_at_ms);

CREATE TABLE approval_grant_audit (
    id TEXT PRIMARY KEY NOT NULL,
    grant_id TEXT NOT NULL REFERENCES approval_grants(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    event TEXT NOT NULL CHECK (event IN ('created','used','denied','expired','revoked','exhausted')),
    tool TEXT,
    path_count INTEGER NOT NULL DEFAULT 0 CHECK (path_count >= 0),
    calls INTEGER NOT NULL DEFAULT 0 CHECK (calls >= 0),
    files_scanned INTEGER NOT NULL DEFAULT 0 CHECK (files_scanned >= 0),
    bytes_read INTEGER NOT NULL DEFAULT 0 CHECK (bytes_read >= 0),
    bytes_written INTEGER NOT NULL DEFAULT 0 CHECK (bytes_written >= 0),
    reason TEXT,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_approval_grant_audit_task ON approval_grant_audit(task_id, created_at_ms, id);

CREATE TRIGGER approval_grant_state_audit
AFTER UPDATE OF state ON approval_grants
WHEN OLD.state = 'active' AND NEW.state IN ('revoked','expired','exhausted')
BEGIN
    INSERT INTO approval_grant_audit(id,grant_id,task_id,event,reason,created_at_ms)
    VALUES(lower(hex(randomblob(16))),NEW.id,NEW.task_id,NEW.state,'grant lifecycle transition',NEW.updated_at_ms);
END;

UPDATE schema_version SET version = 18 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '18' WHERE key = 'schema_version';
