use super::*;

impl ToolCatalogStore for SqliteRepository {
    async fn replace_catalog(
        &self,
        groups: &[ToolGroup],
        tools: &[ToolDefinition],
        presets: &[ToolPreset],
    ) -> Result<(), StorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin catalog sync", error))?;
        for group in groups {
            sqlx::query("INSERT INTO tool_groups(id,key,display_name,sort_order) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,display_name=excluded.display_name,sort_order=excluded.sort_order")
                .bind(&group.id).bind(&group.key).bind(&group.display_name).bind(group.sort_order)
                .execute(&mut *transaction).await.map_err(|error| backend("sync tool group", error))?;
        }
        for tool in tools {
            let capabilities = serde_json::to_string(&tool.capabilities)
                .map_err(|error| backend("serialize tool capabilities", error))?;
            sqlx::query("INSERT INTO tools(id,key,group_id,title,description,input_schema_json,capabilities_json,enabled) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,group_id=excluded.group_id,title=excluded.title,description=excluded.description,input_schema_json=excluded.input_schema_json,capabilities_json=excluded.capabilities_json,enabled=excluded.enabled")
                .bind(&tool.id).bind(&tool.key).bind(&tool.group_id).bind(&tool.title).bind(&tool.description)
                .bind(&tool.input_schema_json).bind(capabilities).bind(tool.enabled)
                .execute(&mut *transaction).await.map_err(|error| backend("sync tool", error))?;
        }
        for preset in presets {
            sqlx::query("INSERT INTO tool_presets(id,key,name,description) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET key=excluded.key,name=excluded.name,description=excluded.description")
                .bind(&preset.id).bind(&preset.key).bind(&preset.name).bind(&preset.description)
                .execute(&mut *transaction).await.map_err(|error| backend("sync preset", error))?;
            sqlx::query("DELETE FROM preset_tools WHERE preset_id=?")
                .bind(&preset.id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| backend("clear preset tools", error))?;
            for tool_id in &preset.tool_ids {
                sqlx::query("INSERT INTO preset_tools(preset_id,tool_id) VALUES(?,?)")
                    .bind(&preset.id)
                    .bind(tool_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| backend("sync preset tool", error))?;
            }
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit catalog sync", error))
    }

    async fn list_tools(&self) -> Result<Vec<ToolDefinition>, StorageError> {
        let rows = sqlx::query("SELECT id,key,group_id,title,description,input_schema_json,capabilities_json,enabled FROM tools ORDER BY key")
            .fetch_all(&self.pool).await.map_err(|error| backend("list tools", error))?;
        rows.iter()
            .map(|row| {
                let raw: String = row
                    .try_get("capabilities_json")
                    .map_err(|error| backend("map capabilities", error))?;
                let capabilities: Vec<ToolCapability> = serde_json::from_str(&raw)
                    .map_err(|error| backend("parse capabilities", error))?;
                Ok(ToolDefinition {
                    id: row
                        .try_get("id")
                        .map_err(|error| backend("map tool id", error))?,
                    key: row
                        .try_get("key")
                        .map_err(|error| backend("map tool key", error))?,
                    group_id: row
                        .try_get("group_id")
                        .map_err(|error| backend("map tool group", error))?,
                    title: row
                        .try_get("title")
                        .map_err(|error| backend("map tool title", error))?,
                    description: row
                        .try_get("description")
                        .map_err(|error| backend("map tool description", error))?,
                    input_schema_json: row
                        .try_get("input_schema_json")
                        .map_err(|error| backend("map tool schema", error))?,
                    capabilities,
                    enabled: row
                        .try_get("enabled")
                        .map_err(|error| backend("map tool state", error))?,
                })
            })
            .collect()
    }

    async fn list_presets(&self) -> Result<Vec<ToolPreset>, StorageError> {
        let rows = sqlx::query("SELECT id,key,name,description FROM tool_presets ORDER BY name,id")
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list tool presets", error))?;
        let mut presets = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row
                .try_get("id")
                .map_err(|error| backend("map preset id", error))?;
            let tool_ids = sqlx::query_scalar(
                "SELECT tool_id FROM preset_tools WHERE preset_id=? ORDER BY tool_id",
            )
            .bind(&id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| backend("list preset tools", error))?;
            presets.push(ToolPreset {
                id,
                key: row
                    .try_get("key")
                    .map_err(|error| backend("map preset key", error))?,
                name: row
                    .try_get("name")
                    .map_err(|error| backend("map preset name", error))?,
                description: row
                    .try_get("description")
                    .map_err(|error| backend("map preset description", error))?,
                tool_ids,
            });
        }
        Ok(presets)
    }

    async fn agent_allowed_tool_ids(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<String>, StorageError> {
        sqlx::query_scalar(
            "SELECT tool_id FROM agent_allowed_tools WHERE agent_id=? ORDER BY tool_id",
        )
        .bind(agent_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| backend("list agent allowlist", error))
    }

    async fn set_agent_allowed_tools(
        &self,
        agent_id: &AgentId,
        tool_ids: &[String],
    ) -> Result<(), StorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin allowlist update", error))?;
        sqlx::query("DELETE FROM agent_allowed_tools WHERE agent_id=?")
            .bind(agent_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("clear allowlist", error))?;
        for tool_id in tool_ids {
            sqlx::query("INSERT INTO agent_allowed_tools(agent_id,tool_id) VALUES(?,?)")
                .bind(agent_id.as_str())
                .bind(tool_id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| map_sqlx_conflict("set agent allowlist", error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit allowlist update", error))
    }

    async fn list_agent_names(&self) -> Result<Vec<AgentName>, StorageError> {
        let rows = sqlx::query(
            "SELECT id,name,enabled,sort_order FROM agent_names ORDER BY sort_order,id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| backend("list agent names", error))?;
        rows.iter()
            .map(|row| {
                Ok(AgentName {
                    id: row
                        .try_get("id")
                        .map_err(|error| backend("map agent name id", error))?,
                    name: row
                        .try_get("name")
                        .map_err(|error| backend("map agent name", error))?,
                    enabled: row
                        .try_get("enabled")
                        .map_err(|error| backend("map agent name state", error))?,
                    sort_order: row
                        .try_get("sort_order")
                        .map_err(|error| backend("map agent name order", error))?,
                })
            })
            .collect()
    }
}
