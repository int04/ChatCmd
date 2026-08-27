use super::*;

impl SettingsStore for SqliteRepository {
    async fn setting(&self, key: &str) -> Result<Option<Setting>, StorageError> {
        let row = sqlx::query("SELECT key,value_json,updated_at_ms FROM settings WHERE key=?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| backend("read setting", error))?;
        row.as_ref()
            .map(|item| {
                Ok(Setting {
                    key: item
                        .try_get("key")
                        .map_err(|error| backend("map setting key", error))?,
                    value_json: item
                        .try_get("value_json")
                        .map_err(|error| backend("map setting value", error))?,
                    updated_at_ms: item
                        .try_get("updated_at_ms")
                        .map_err(|error| backend("map setting timestamp", error))?,
                })
            })
            .transpose()
    }

    async fn set_setting(&self, setting: &Setting) -> Result<(), StorageError> {
        serde_json::from_str::<serde_json::Value>(&setting.value_json)
            .map_err(|error| backend("validate setting JSON", error))?;
        sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at_ms=excluded.updated_at_ms")
            .bind(&setting.key).bind(&setting.value_json).bind(setting.updated_at_ms).execute(&self.pool).await
            .map_err(|error| backend("write setting", error))?;
        Ok(())
    }

    async fn execution_mode(
        &self,
        task_id: Option<&TaskId>,
    ) -> Result<ExecutionMode, StorageError> {
        if let Some(task_id) = task_id {
            let mode: Option<String> =
                sqlx::query_scalar("SELECT mode FROM task_execution_modes WHERE task_id=?")
                    .bind(task_id.as_str())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| backend("read task execution mode", error))?;
            if let Some(mode) = mode {
                return ExecutionMode::from_str(&mode).map_err(invalid_data);
            }
        }
        let setting = self.setting("command_execution_mode").await?;
        match setting {
            Some(value) => {
                let mode: String = serde_json::from_str(&value.value_json)
                    .map_err(|error| backend("parse execution mode setting", error))?;
                ExecutionMode::from_str(&mode).map_err(invalid_data)
            }
            None => Ok(ExecutionMode::Approval),
        }
    }
}

impl ArtifactStore for SqliteRepository {
    async fn register_artifact(&self, artifact: &Artifact) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO artifact_registry(id,task_id,session_id,relative_path,media_type,size_bytes,sha256_hex,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET relative_path=excluded.relative_path,media_type=excluded.media_type,size_bytes=excluded.size_bytes,sha256_hex=excluded.sha256_hex,updated_at_ms=excluded.updated_at_ms")
            .bind(artifact.id.as_str()).bind(artifact.task_id.as_str()).bind(artifact.session_id.as_ref().map(SessionId::as_str))
            .bind(&artifact.relative_path).bind(&artifact.media_type).bind(artifact.size_bytes).bind(&artifact.sha256_hex)
            .bind(artifact.created_at_ms).bind(artifact.updated_at_ms).execute(&self.pool).await
            .map_err(|error| map_sqlx_conflict("register artifact", error))?;
        Ok(())
    }

    async fn artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>, StorageError> {
        let row = sqlx::query("SELECT id,task_id,session_id,relative_path,media_type,size_bytes,sha256_hex,created_at_ms,updated_at_ms FROM artifact_registry WHERE id=?")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|error| backend("read artifact", error))?;
        row.as_ref()
            .map(|item| {
                Ok(Artifact {
                    id: ArtifactId::new(
                        item.try_get::<String, _>("id")
                            .map_err(|error| backend("map artifact id", error))?,
                    )
                    .map_err(invalid_data)?,
                    task_id: TaskId::new(
                        item.try_get::<String, _>("task_id")
                            .map_err(|error| backend("map artifact task", error))?,
                    )
                    .map_err(invalid_data)?,
                    session_id: item
                        .try_get::<Option<String>, _>("session_id")
                        .map_err(|error| backend("map artifact session", error))?
                        .map(SessionId::new)
                        .transpose()
                        .map_err(invalid_data)?,
                    relative_path: item
                        .try_get("relative_path")
                        .map_err(|error| backend("map artifact path", error))?,
                    media_type: item
                        .try_get("media_type")
                        .map_err(|error| backend("map artifact media type", error))?,
                    size_bytes: item
                        .try_get("size_bytes")
                        .map_err(|error| backend("map artifact size", error))?,
                    sha256_hex: item
                        .try_get("sha256_hex")
                        .map_err(|error| backend("map artifact hash", error))?,
                    created_at_ms: item
                        .try_get("created_at_ms")
                        .map_err(|error| backend("map artifact timestamp", error))?,
                    updated_at_ms: item
                        .try_get("updated_at_ms")
                        .map_err(|error| backend("map artifact timestamp", error))?,
                })
            })
            .transpose()
    }
}
