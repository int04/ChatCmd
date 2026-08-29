ALTER TABLE tasks ADD COLUMN allow_execute INTEGER DEFAULT 1 CHECK (allow_execute IS NULL OR allow_execute IN (0, 1));
UPDATE tasks SET allow_execute = 1;

UPDATE schema_version SET version = 5 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '5' WHERE key = 'schema_version';
