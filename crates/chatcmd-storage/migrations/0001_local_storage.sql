CREATE TABLE app_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE schema_version (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    version INTEGER NOT NULL CHECK (version >= 0)
) STRICT;
INSERT INTO schema_version(singleton_id, version) VALUES (1, 1);
INSERT INTO app_metadata(key, value) VALUES ('schema_version', '1');

CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE migration_sources (
    source_key TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('imported', 'imported_with_warnings', 'failed')),
    warning_count INTEGER NOT NULL DEFAULT 0 CHECK (warning_count >= 0),
    imported_at_ms INTEGER NOT NULL,
    error TEXT,
    UNIQUE(path, fingerprint)
) STRICT;

CREATE TABLE local_device (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    device_id TEXT NOT NULL UNIQUE,
    installation_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    os_version TEXT,
    architecture TEXT NOT NULL,
    app_version TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE mcp_agents (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    secret_hash BLOB NOT NULL UNIQUE CHECK (length(secret_hash) = 32),
    secret_last4 TEXT NOT NULL CHECK (length(secret_last4) = 4),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    project_folder TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER
) STRICT;
CREATE INDEX idx_mcp_agents_enabled ON mcp_agents(enabled);

CREATE TABLE tool_groups (
    id TEXT PRIMARY KEY NOT NULL,
    key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    sort_order INTEGER NOT NULL
) STRICT;

CREATE TABLE tools (
    id TEXT PRIMARY KEY NOT NULL,
    key TEXT NOT NULL UNIQUE,
    group_id TEXT NOT NULL REFERENCES tool_groups(id) ON DELETE RESTRICT,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    input_schema_json TEXT NOT NULL CHECK (json_valid(input_schema_json)),
    capabilities_json TEXT NOT NULL CHECK (json_valid(capabilities_json)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
) STRICT;
CREATE INDEX idx_tools_group ON tools(group_id, key);

CREATE TABLE tool_presets (
    id TEXT PRIMARY KEY NOT NULL,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL
) STRICT;

CREATE TABLE preset_tools (
    preset_id TEXT NOT NULL REFERENCES tool_presets(id) ON DELETE CASCADE,
    tool_id TEXT NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    PRIMARY KEY (preset_id, tool_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE agent_allowed_tools (
    agent_id TEXT NOT NULL REFERENCES mcp_agents(id) ON DELETE CASCADE,
    tool_id TEXT NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    PRIMARY KEY (agent_id, tool_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE agent_names (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    sort_order INTEGER NOT NULL
) STRICT;

CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT REFERENCES mcp_agents(id) ON DELETE SET NULL,
    device_id TEXT NOT NULL REFERENCES local_device(device_id) ON DELETE RESTRICT,
    conversation_scope_hash TEXT,
    title TEXT,
    source TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending','running','completed','failed','stopped','interrupted')),
    active_session_id TEXT,
    generation INTEGER NOT NULL DEFAULT 1 CHECK (generation > 0),
    stopped_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_tasks_status_updated ON tasks(status, updated_at_ms DESC);
CREATE INDEX idx_tasks_agent_updated ON tasks(agent_id, updated_at_ms DESC);

CREATE TABLE terminal_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    turn_id TEXT,
    executable TEXT NOT NULL,
    working_directory TEXT NOT NULL,
    columns INTEGER NOT NULL CHECK (columns > 0),
    rows INTEGER NOT NULL CHECK (rows > 0),
    process_id INTEGER,
    status TEXT NOT NULL CHECK (status IN ('starting','running','exited','failed','closed','interrupted')),
    exit_code INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    closed_at_ms INTEGER
) STRICT;
CREATE INDEX idx_terminal_sessions_task ON terminal_sessions(task_id, created_at_ms);
CREATE INDEX idx_terminal_sessions_status ON terminal_sessions(status, updated_at_ms);

CREATE TABLE task_sessions (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    session_id TEXT NOT NULL REFERENCES terminal_sessions(id) ON DELETE RESTRICT,
    generation INTEGER NOT NULL CHECK (generation > 0),
    replaced_session_id TEXT REFERENCES terminal_sessions(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('starting','running','exited','failed','closed','interrupted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (task_id, session_id),
    UNIQUE(task_id, generation)
) WITHOUT ROWID, STRICT;

CREATE TABLE turn_bindings (
    agent_id TEXT NOT NULL REFERENCES mcp_agents(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_device(device_id) ON DELETE RESTRICT,
    turn_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    last_used_at_ms INTEGER NOT NULL,
    PRIMARY KEY (agent_id, device_id, turn_id)
) WITHOUT ROWID, STRICT;
CREATE INDEX idx_turn_bindings_task ON turn_bindings(task_id);

CREATE TABLE terminal_event_chunks (
    session_id TEXT NOT NULL REFERENCES terminal_sessions(id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    event_id TEXT NOT NULL UNIQUE,
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    turn_id TEXT,
    kind TEXT NOT NULL,
    stream TEXT,
    payload BLOB NOT NULL CHECK (length(payload) <= 65536),
    payload_encoding TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, sequence)
) WITHOUT ROWID, STRICT;
CREATE INDEX idx_terminal_chunks_task ON terminal_event_chunks(task_id, created_at_ms);

CREATE TABLE timeline_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    turn_id TEXT,
    session_id TEXT REFERENCES terminal_sessions(id) ON DELETE SET NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    metadata_json TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_timeline_task_created ON timeline_events(task_id, created_at_ms, event_id);

CREATE TABLE approvals (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    session_id TEXT REFERENCES terminal_sessions(id) ON DELETE SET NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','approved','rejected','expired','cancelled')),
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    decision_json TEXT CHECK (decision_json IS NULL OR json_valid(decision_json)),
    created_at_ms INTEGER NOT NULL,
    resolved_at_ms INTEGER
) STRICT;
CREATE INDEX idx_approvals_task_state ON approvals(task_id, state, created_at_ms);

CREATE TABLE task_execution_modes (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    mode TEXT NOT NULL CHECK (mode IN ('approval','allow','deny')),
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE artifact_registry (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    session_id TEXT REFERENCES terminal_sessions(id) ON DELETE SET NULL,
    relative_path TEXT NOT NULL,
    media_type TEXT,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256_hex TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(task_id, relative_path)
) STRICT;
CREATE INDEX idx_artifacts_session ON artifact_registry(session_id);
