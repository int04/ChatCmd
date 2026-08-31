ALTER TABLE workspace_projects ADD COLUMN sort_order INTEGER;

WITH ordered AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY updated_at_ms DESC, name COLLATE NOCASE) - 1 AS position
    FROM workspace_projects
)
UPDATE workspace_projects
SET sort_order = (SELECT position FROM ordered WHERE ordered.id = workspace_projects.id);
