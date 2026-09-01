CREATE TABLE chatgpt_message_queue (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('queued','immediate')),
    sort_order INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_chatgpt_message_queue_task
    ON chatgpt_message_queue(task_id, sort_order, created_at_ms);
CREATE INDEX idx_chatgpt_message_queue_immediate
    ON chatgpt_message_queue(task_id, mode, sort_order, created_at_ms);

UPDATE schema_version SET version = 13 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '13' WHERE key = 'schema_version';
