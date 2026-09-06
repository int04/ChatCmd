-- Durable compact jobs. A revision is also the checkpoint operation sequence.
CREATE TABLE chatgpt_compact_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    phase TEXT NOT NULL CHECK (phase IN ('preparing','writing_handoff','saving_handoff','opening_new_chat','completed','cancelled')),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    start_operation_id TEXT NOT NULL UNIQUE,
    old_conversation_id TEXT NOT NULL,
    old_conversation_url TEXT NOT NULL,
    old_model TEXT NOT NULL,
    old_request_id TEXT,
    old_scope_hash TEXT,
    old_active_session_id TEXT,
    old_request_params_json TEXT CHECK (old_request_params_json IS NULL OR json_valid(old_request_params_json)),
    agent_name TEXT,
    project_folder TEXT,
    new_conversation_id TEXT,
    new_conversation_url TEXT,
    new_scope_hash TEXT,
    handoff_text TEXT CHECK (handoff_text IS NULL OR length(CAST(handoff_text AS BLOB)) <= 524288),
    detail TEXT CHECK (detail IS NULL OR length(detail) <= 2000),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    CHECK ((new_conversation_id IS NULL) = (new_conversation_url IS NULL)),
    CHECK (phase <> 'completed' OR (handoff_text IS NOT NULL AND new_conversation_id IS NOT NULL AND completed_at_ms IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX idx_compact_single_active ON chatgpt_compact_jobs(task_id)
    WHERE phase NOT IN ('completed','cancelled');
CREATE UNIQUE INDEX idx_compact_destination ON chatgpt_compact_jobs(new_conversation_id)
    WHERE phase NOT IN ('completed','cancelled') AND new_conversation_id IS NOT NULL;
CREATE INDEX idx_compact_history ON chatgpt_compact_jobs(task_id, created_at_ms DESC, id DESC);

CREATE TABLE chatgpt_compact_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES chatgpt_compact_jobs(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    phase TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(job_id, revision)
) STRICT;

-- Tombstones survive reload/restart and are consulted before browser enrollment.
CREATE TABLE chatgpt_compact_archives (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    scope_hash TEXT NOT NULL,
    previous_scope_hash TEXT,
    old_session_id TEXT,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    job_id TEXT NOT NULL REFERENCES chatgpt_compact_jobs(id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_compact_archived_scope ON chatgpt_compact_archives(scope_hash);
CREATE INDEX idx_compact_archived_previous_scope ON chatgpt_compact_archives(previous_scope_hash);
CREATE TABLE chatgpt_compact_obsolete_requests (
    request_id TEXT PRIMARY KEY NOT NULL REFERENCES chatgpt_bridge_requests(id) ON DELETE CASCADE,
    job_id TEXT NOT NULL REFERENCES chatgpt_compact_jobs(id) ON DELETE CASCADE
) STRICT;

-- Defense in depth for non-HTTP identity writers and callbacks racing completion.
-- These guards affect compacted conversations only, not ordinary identity recovery.
CREATE TRIGGER compact_archived_task_insert BEFORE INSERT ON tasks
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_archives a
    WHERE a.scope_hash=NEW.conversation_scope_hash OR a.previous_scope_hash=NEW.conversation_scope_hash)
BEGIN SELECT RAISE(ABORT, 'compact: archived conversation scope'); END;
CREATE TRIGGER compact_archived_task_update BEFORE UPDATE OF conversation_scope_hash ON tasks
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_archives a
    WHERE a.scope_hash=NEW.conversation_scope_hash OR a.previous_scope_hash=NEW.conversation_scope_hash)
BEGIN SELECT RAISE(ABORT, 'compact: archived conversation scope'); END;
CREATE TRIGGER compact_task_generation BEFORE UPDATE OF generation,active_session_id ON tasks
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_archives a WHERE a.task_id=NEW.id)
    AND (NEW.generation < OLD.generation OR EXISTS (SELECT 1 FROM chatgpt_compact_archives a
         WHERE a.task_id=NEW.id AND a.old_session_id=NEW.active_session_id))
BEGIN SELECT RAISE(ABORT, 'compact: obsolete task session'); END;
CREATE TRIGGER compact_task_scope_lock BEFORE UPDATE OF conversation_scope_hash ON tasks
WHEN NEW.conversation_scope_hash IS NOT OLD.conversation_scope_hash
    AND EXISTS (SELECT 1 FROM chatgpt_compact_jobs j WHERE j.task_id=NEW.id AND j.phase NOT IN ('completed','cancelled'))
BEGIN SELECT RAISE(ABORT, 'compact: task binding is locked'); END;
CREATE TRIGGER compact_reserved_task_insert BEFORE INSERT ON tasks
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_jobs j WHERE j.phase NOT IN ('completed','cancelled')
    AND j.task_id<>NEW.id AND (j.old_scope_hash=NEW.conversation_scope_hash OR j.new_scope_hash=NEW.conversation_scope_hash))
BEGIN SELECT RAISE(ABORT, 'compact: reserved conversation scope'); END;

CREATE TRIGGER compact_archived_conversation_insert BEFORE INSERT ON chatgpt_conversations
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=NEW.conversation_id)
BEGIN SELECT RAISE(ABORT, 'compact: archived conversation'); END;
CREATE TRIGGER compact_archived_conversation_update BEFORE UPDATE OF conversation_id ON chatgpt_conversations
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=NEW.conversation_id)
BEGIN SELECT RAISE(ABORT, 'compact: archived conversation'); END;
CREATE TRIGGER compact_binding_lock BEFORE UPDATE OF conversation_id,conversation_url ON chatgpt_conversations
WHEN (NEW.conversation_id IS NOT OLD.conversation_id OR NEW.conversation_url IS NOT OLD.conversation_url)
    AND EXISTS (SELECT 1 FROM chatgpt_compact_jobs j WHERE j.task_id=NEW.task_id AND j.phase NOT IN ('completed','cancelled'))
BEGIN SELECT RAISE(ABORT, 'compact: task binding is locked'); END;
CREATE TRIGGER compact_obsolete_request_update BEFORE UPDATE OF status,task_id,conversation_id,conversation_url ON chatgpt_bridge_requests
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_obsolete_requests WHERE request_id=OLD.id)
    AND NEW.task_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'compact: obsolete bridge request'); END;
CREATE TRIGGER compact_request_insert BEFORE INSERT ON chatgpt_bridge_requests
WHEN EXISTS (SELECT 1 FROM chatgpt_compact_jobs j WHERE j.phase NOT IN ('completed','cancelled')
    AND (j.task_id=NEW.task_id OR j.new_conversation_id=NEW.conversation_id OR j.old_conversation_id=NEW.conversation_id))
    OR EXISTS (SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=NEW.conversation_id)
BEGIN SELECT RAISE(ABORT, 'compact: message dispatch is paused'); END;

-- Preserve content/order while preventing MCP claim_immediate from consuming it.
CREATE TRIGGER compact_queue_insert AFTER INSERT ON chatgpt_message_queue
WHEN NEW.mode='immediate' AND EXISTS (SELECT 1 FROM chatgpt_compact_jobs j
    WHERE j.task_id=NEW.task_id AND j.phase NOT IN ('completed','cancelled'))
BEGIN UPDATE chatgpt_message_queue SET mode='queued' WHERE id=NEW.id; END;
CREATE TRIGGER compact_queue_update AFTER UPDATE OF mode ON chatgpt_message_queue
WHEN NEW.mode='immediate' AND EXISTS (SELECT 1 FROM chatgpt_compact_jobs j
    WHERE j.task_id=NEW.task_id AND j.phase NOT IN ('completed','cancelled'))
BEGIN UPDATE chatgpt_message_queue SET mode='queued' WHERE id=NEW.id; END;

UPDATE schema_version SET version = 23 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '23' WHERE key = 'schema_version';
