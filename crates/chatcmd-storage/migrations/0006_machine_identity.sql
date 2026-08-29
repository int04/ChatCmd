ALTER TABLE local_device ADD COLUMN machine_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_local_device_machine_id ON local_device(machine_id) WHERE machine_id IS NOT NULL;

UPDATE schema_version SET version = 6 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '6' WHERE key = 'schema_version';
