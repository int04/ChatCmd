ALTER TABLE subagent_runs ADD COLUMN fallback_state TEXT NOT NULL DEFAULT 'none' CHECK (fallback_state IN ('none','requested','started','claimed','exhausted'));
ALTER TABLE subagent_runs ADD COLUMN fallback_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE subagent_runs ADD COLUMN fallback_error TEXT;
ALTER TABLE subagent_runs ADD COLUMN fallback_conversation_id TEXT;
ALTER TABLE subagent_runs ADD COLUMN fallback_conversation_url TEXT;
CREATE INDEX idx_subagent_runs_fallback ON subagent_runs(fallback_state,status,updated_at_ms DESC);

UPDATE schema_version SET version = 14 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '14' WHERE key = 'schema_version';
