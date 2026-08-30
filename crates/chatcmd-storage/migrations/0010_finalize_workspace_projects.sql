UPDATE schema_version SET version = 10 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '10' WHERE key = 'schema_version';
