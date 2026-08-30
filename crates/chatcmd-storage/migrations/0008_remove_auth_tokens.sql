DROP TABLE IF EXISTS local_auth_session;

UPDATE schema_version SET version = 8 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '8' WHERE key = 'schema_version';
