CREATE TABLE IF NOT EXISTS local_auth_session (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    access_token_expires_at TEXT NOT NULL,
    refresh_token_expires_at TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

UPDATE schema_version SET version = 7 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '7' WHERE key = 'schema_version';
