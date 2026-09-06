-- Continuation is a per-job user choice, never inferred for existing jobs.
ALTER TABLE chatgpt_compact_jobs ADD COLUMN continue_after_compact INTEGER NOT NULL DEFAULT 0 CHECK (continue_after_compact IN (0,1));

UPDATE schema_version SET version = 24 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '24' WHERE key = 'schema_version';
