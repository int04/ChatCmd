CREATE TABLE workspace_repository_indexes (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    schema_version INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    state TEXT NOT NULL CHECK (state IN ('building','fresh','stale','unknown','failed')),
    ignore_fingerprint TEXT NOT NULL,
    entry_count INTEGER NOT NULL DEFAULT 0 CHECK (entry_count >= 0),
    indexed_bytes INTEGER NOT NULL DEFAULT 0 CHECK (indexed_bytes >= 0),
    last_reconciled_at_ms INTEGER,
    last_error TEXT,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE workspace_repository_index_entries (
    workspace_id TEXT NOT NULL REFERENCES workspace_repository_indexes(workspace_id) ON DELETE CASCADE,
    relative_path_bytes BLOB NOT NULL,
    display_path TEXT NOT NULL,
    normalized_path TEXT NOT NULL,
    normalized_extension TEXT,
    entry_type TEXT NOT NULL CHECK (entry_type IN ('file','directory','symlink','other')),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    modified_at_ns TEXT,
    file_identity TEXT,
    version_token_metadata TEXT,
    ignored_state INTEGER NOT NULL DEFAULT 0 CHECK (ignored_state IN (0,1)),
    last_seen_generation INTEGER NOT NULL CHECK (last_seen_generation >= 0),
    PRIMARY KEY (workspace_id, relative_path_bytes)
) STRICT;

CREATE INDEX idx_workspace_repository_entries_path ON workspace_repository_index_entries(workspace_id, normalized_path);
CREATE INDEX idx_workspace_repository_entries_extension ON workspace_repository_index_entries(workspace_id, normalized_extension, normalized_path);
CREATE INDEX idx_workspace_repository_entries_generation ON workspace_repository_index_entries(workspace_id, last_seen_generation);

UPDATE schema_version SET version = 17 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '17' WHERE key = 'schema_version';
