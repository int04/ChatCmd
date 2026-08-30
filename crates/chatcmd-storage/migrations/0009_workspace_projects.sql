CREATE TABLE workspace_projects (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    canonical_path TEXT NOT NULL UNIQUE,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

ALTER TABLE tasks ADD COLUMN project_folder TEXT;
ALTER TABLE chatgpt_bridge_requests ADD COLUMN project_folder TEXT;

UPDATE tasks
SET project_folder = (
    SELECT mcp_agents.project_folder
    FROM mcp_agents
    WHERE mcp_agents.id = tasks.agent_id
)
WHERE project_folder IS NULL
  AND agent_id IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM mcp_agents
      WHERE mcp_agents.id = tasks.agent_id
        AND mcp_agents.project_folder IS NOT NULL
        AND trim(mcp_agents.project_folder) <> ''
  );

CREATE INDEX idx_tasks_project_folder_updated
ON tasks(project_folder, updated_at_ms DESC);
