ALTER TABLE subagent_runs
ADD COLUMN requested_approval_grant_json TEXT
CHECK (
    requested_approval_grant_json IS NULL
    OR json_valid(requested_approval_grant_json)
);

UPDATE schema_version SET version = 20 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '20' WHERE key = 'schema_version';
