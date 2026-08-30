use super::*;

impl McpAgentStore for SqliteRepository {
    async fn create_agent(&self, input: NewMcpAgent) -> Result<AgentSecretResult, StorageError> {
        let raw = generate_secret();
        let digest = SecretHash::from_token(&raw);
        let generated = GeneratedSecret::new(raw);
        let id = input.id.unwrap_or_else(|| {
            AgentId::new(uuid::Uuid::new_v4().to_string()).expect("UUID is non-empty")
        });
        let now = now_ms()?;
        let result = sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?)")
            .bind(id.as_str()).bind(input.name.trim()).bind(digest.as_bytes().as_slice()).bind(generated.last4())
            .bind(input.enabled).bind(now).bind(now)
            .execute(&self.pool).await;
        if let Err(error) = result {
            return Err(map_sqlx_conflict("create MCP agent", error));
        }
        let agent = McpAgent {
            id,
            name: input.name.trim().to_owned(),
            enabled: input.enabled,
            secret_last4: generated.last4().to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
            last_used_at_ms: None,
        };
        Ok(AgentSecretResult {
            agent,
            secret: generated,
        })
    }

    async fn list_agents(&self) -> Result<Vec<McpAgent>, StorageError> {
        let rows = sqlx::query("SELECT id,name,enabled,secret_last4,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents ORDER BY name COLLATE NOCASE")
            .fetch_all(&self.pool).await.map_err(|error| backend("list MCP agents", error))?;
        rows.iter().map(map_agent).collect()
    }

    async fn agent(&self, id: &AgentId) -> Result<Option<McpAgent>, StorageError> {
        let row = sqlx::query("SELECT id,name,enabled,secret_last4,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read MCP agent", error))?;
        row.as_ref().map(map_agent).transpose()
    }

    async fn rotate_agent_secret(&self, id: &AgentId) -> Result<AgentSecretResult, StorageError> {
        let raw = generate_secret();
        let digest = SecretHash::from_token(&raw);
        let generated = GeneratedSecret::new(raw);
        let now = now_ms()?;
        let affected = sqlx::query(
            "UPDATE mcp_agents SET secret_hash=?, secret_last4=?, updated_at_ms=? WHERE id=?",
        )
        .bind(digest.as_bytes().as_slice())
        .bind(generated.last4())
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| backend("rotate MCP agent secret", error))?
        .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        let agent = self
            .agent(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("MCP agent {id}")))?;
        Ok(AgentSecretResult {
            agent,
            secret: generated,
        })
    }

    async fn set_agent_enabled(&self, id: &AgentId, enabled: bool) -> Result<(), StorageError> {
        let affected = sqlx::query("UPDATE mcp_agents SET enabled=?,updated_at_ms=? WHERE id=?")
            .bind(enabled)
            .bind(now_ms()?)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| backend("set MCP agent state", error))?
            .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        Ok(())
    }

    async fn update_agent(
        &self,
        id: &AgentId,
        input: NewMcpAgent,
    ) -> Result<McpAgent, StorageError> {
        let affected =
            sqlx::query("UPDATE mcp_agents SET name=?,enabled=?,updated_at_ms=? WHERE id=?")
                .bind(input.name.trim())
                .bind(input.enabled)
                .bind(now_ms()?)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(|error| map_sqlx_conflict("update MCP agent", error))?
                .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        self.agent(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("MCP agent {id}")))
    }

    async fn delete_agent(&self, id: &AgentId) -> Result<(), StorageError> {
        let affected = sqlx::query("DELETE FROM mcp_agents WHERE id=?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| map_sqlx_conflict("delete MCP agent", error))?
            .rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound(format!("MCP agent {id}")));
        }
        Ok(())
    }
}

impl PolicyLookup for SqliteRepository {
    async fn lookup_policy_by_token(
        &self,
        raw_token: &str,
    ) -> Result<Option<McpAgentPolicy>, StorageError> {
        let candidate = SecretHash::from_token(raw_token);
        let row = sqlx::query("SELECT id,name,enabled,secret_last4,secret_hash,created_at_ms,updated_at_ms,last_used_at_ms FROM mcp_agents WHERE secret_hash=? AND enabled=1")
            .bind(candidate.as_bytes().as_slice()).fetch_optional(&self.pool).await
            .map_err(|error| backend("lookup MCP path token", error))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored = SecretHash::from_bytes(
            row.try_get::<Vec<u8>, _>("secret_hash")
                .map_err(|error| backend("map secret hash", error))?
                .as_slice(),
        )
        .map_err(invalid_data)?;
        if !stored.constant_time_eq(&candidate) {
            return Ok(None);
        }
        let agent = map_agent(&row)?;
        let tool_rows = sqlx::query("SELECT tools.key FROM tools JOIN agent_allowed_tools ON agent_allowed_tools.tool_id=tools.id WHERE agent_allowed_tools.agent_id=? AND tools.enabled=1 ORDER BY tools.key")
            .bind(agent.id.as_str()).fetch_all(&self.pool).await.map_err(|error| backend("read agent allowlist", error))?;
        let allowed_tool_keys = tool_rows
            .iter()
            .map(|item| {
                item.try_get::<String, _>("key")
                    .map_err(|error| backend("map tool key", error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        sqlx::query("UPDATE mcp_agents SET last_used_at_ms=? WHERE id=?")
            .bind(now_ms()?)
            .bind(agent.id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| backend("update agent last use", error))?;
        Ok(Some(McpAgentPolicy {
            agent,
            allowed_tool_keys,
        }))
    }
}
