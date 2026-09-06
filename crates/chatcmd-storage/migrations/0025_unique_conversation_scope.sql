-- Repair legacy split-brain top-level tasks before enforcing one owner per provider scope.
WITH ranked_scope_owners AS (
    SELECT
        tasks.id,
        ROW_NUMBER() OVER (
            PARTITION BY tasks.agent_id, tasks.conversation_scope_hash
            ORDER BY
                CASE WHEN EXISTS (
                    SELECT 1 FROM chatgpt_conversations
                    WHERE chatgpt_conversations.task_id = tasks.id
                ) THEN 0 ELSE 1 END,
                CASE tasks.source WHEN 'chatgpt_web' THEN 0 ELSE 1 END,
                tasks.created_at_ms,
                tasks.id
        ) AS owner_rank
    FROM tasks
    WHERE tasks.agent_id IS NOT NULL
      AND tasks.conversation_scope_hash IS NOT NULL
      AND trim(tasks.conversation_scope_hash) <> ''
      AND tasks.source IN ('chatgpt_web', 'mcp')
)
UPDATE tasks
SET conversation_scope_hash = NULL
WHERE id IN (
    SELECT id FROM ranked_scope_owners WHERE owner_rank > 1
);

CREATE UNIQUE INDEX idx_tasks_top_level_conversation_scope
ON tasks(agent_id, conversation_scope_hash)
WHERE agent_id IS NOT NULL
  AND conversation_scope_hash IS NOT NULL
  AND trim(conversation_scope_hash) <> ''
  AND source IN ('chatgpt_web', 'mcp');

UPDATE schema_version SET version = 25 WHERE singleton_id = 1;
UPDATE app_metadata SET value = '25' WHERE key = 'schema_version';
