CREATE TABLE task_sessions_v2 (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    replaced_session_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('starting','running','exited','failed','closed','interrupted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (task_id, session_id),
    UNIQUE(task_id, generation)
) WITHOUT ROWID, STRICT;

INSERT INTO task_sessions_v2
SELECT task_id, session_id, generation, replaced_session_id, status, created_at_ms, updated_at_ms
FROM task_sessions;
DROP TABLE task_sessions;
ALTER TABLE task_sessions_v2 RENAME TO task_sessions;
CREATE INDEX idx_task_sessions_updated ON task_sessions(updated_at_ms DESC);

ALTER TABLE timeline_events RENAME TO timeline_events_v1;
CREATE TABLE timeline_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    turn_id TEXT,
    session_id TEXT,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    metadata_json TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
    created_at_ms INTEGER NOT NULL
) STRICT;
INSERT INTO timeline_events
SELECT event_id, task_id, turn_id, session_id, actor, kind, idempotency_key,
       payload_json, metadata_json, created_at_ms
FROM timeline_events_v1;
DROP TABLE timeline_events_v1;
CREATE INDEX idx_timeline_task_created ON timeline_events(task_id, created_at_ms, event_id);
CREATE INDEX idx_timeline_session_created ON timeline_events(session_id, created_at_ms, event_id);

UPDATE schema_version SET version = 2 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '2' WHERE key = 'schema_version';
