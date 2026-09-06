CREATE TABLE filesystem_operation_journal (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_type TEXT NOT NULL CHECK (operation_type IN ('copy', 'move', 'delete')),
    owner_agent_id TEXT NOT NULL,
    owner_task_id TEXT,
    source_path TEXT NOT NULL,
    destination_path TEXT,
    staging_path TEXT,
    backup_path TEXT,
    requested_options_json TEXT NOT NULL,
    counters_json TEXT NOT NULL,
    phase TEXT NOT NULL,
    rollback_actions_json TEXT NOT NULL,
    error_json TEXT,
    lease_expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX filesystem_operation_journal_recovery
    ON filesystem_operation_journal(phase, lease_expires_at_ms, updated_at_ms);

UPDATE schema_version SET version = 15 WHERE singleton_id = 1;
