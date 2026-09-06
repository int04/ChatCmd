-- Plan 13: derived metadata for bounded/lazy timeline queries without rewriting legacy rows.
-- VIRTUAL generated columns preserve backward compatibility and avoid duplicating payload bytes.
ALTER TABLE timeline_events ADD COLUMN payload_size_bytes INTEGER
    GENERATED ALWAYS AS (length(CAST(payload_json AS BLOB))) VIRTUAL;
ALTER TABLE timeline_events ADD COLUMN payload_truncated INTEGER
    GENERATED ALWAYS AS (
        CASE
            WHEN COALESCE(json_extract(payload_json, '$.inputTruncated'), 0) <> 0
              OR COALESCE(json_extract(payload_json, '$.outputTruncated'), 0) <> 0
              OR COALESCE(json_extract(payload_json, '$.externalizationFailed'), 0) <> 0
            THEN 1 ELSE 0
        END
    ) VIRTUAL;
ALTER TABLE timeline_events ADD COLUMN artifact_id TEXT
    GENERATED ALWAYS AS (json_extract(payload_json, '$.artifactRef')) VIRTUAL;
ALTER TABLE timeline_events ADD COLUMN schema_version INTEGER
    GENERATED ALWAYS AS (COALESCE(json_extract(payload_json, '$.schemaVersion'), 1)) VIRTUAL;

CREATE INDEX IF NOT EXISTS idx_timeline_events_task_schema
    ON timeline_events(task_id, schema_version, created_at_ms, event_id);
CREATE INDEX IF NOT EXISTS idx_timeline_events_artifact
    ON timeline_events(artifact_id)
    WHERE artifact_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_timeline_events_large_payload
    ON timeline_events(task_id, payload_size_bytes)
    WHERE payload_size_bytes >= 131072;
