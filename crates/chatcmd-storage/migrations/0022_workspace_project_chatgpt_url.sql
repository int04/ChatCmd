ALTER TABLE workspace_projects ADD COLUMN chatgpt_project_url TEXT;

UPDATE schema_version SET version = 22 WHERE singleton_id = 1;
