use super::*;

impl SqliteRepository {
    pub async fn upsert_filesystem_operation_journal_json(
        &self,
        journal_json: &str,
    ) -> Result<(), StorageError> {
        let value: serde_json::Value = serde_json::from_str(journal_json)
            .map_err(|error| invalid_data(format!("invalid filesystem journal JSON: {error}")))?;
        let required = |key: &str| -> Result<&str, StorageError> {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    StorageError::InvalidData(format!("filesystem journal missing {key}"))
                })
        };
        let operation_id = required("operationId")?;
        let operation_type = required("operationType")?;
        let owner_agent_id = required("ownerAgent")?;
        let source_path = required("source")?;
        let phase = required("phase")?;
        let destination_path = value.get("destination").and_then(serde_json::Value::as_str);
        let staging_path = value.get("stagingPath").and_then(serde_json::Value::as_str);
        let backup_path = value.get("backupPath").and_then(serde_json::Value::as_str);
        let owner_task_id = value.get("ownerTask").and_then(serde_json::Value::as_str);
        let requested_options_json = serde_json::to_string(
            value
                .get("requestedOptions")
                .unwrap_or(&serde_json::Value::Null),
        )
        .map_err(invalid_data)?;
        let counters_json =
            serde_json::to_string(value.get("counts").unwrap_or(&serde_json::Value::Null))
                .map_err(invalid_data)?;
        let rollback_actions_json = serde_json::to_string(
            value
                .get("rollbackActions")
                .unwrap_or(&serde_json::Value::Null),
        )
        .map_err(invalid_data)?;
        let error_json = value
            .get("error")
            .filter(|item| !item.is_null())
            .map(serde_json::to_string)
            .transpose()
            .map_err(invalid_data)?;
        let updated_at_ms = value
            .get("updatedAtUnixMs")
            .and_then(serde_json::Value::as_u64)
            .and_then(|item| i64::try_from(item).ok())
            .unwrap_or(now_ms()?);
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin filesystem journal transition", error))?;
        sqlx::query("INSERT INTO filesystem_operation_journal(operation_id,operation_type,owner_agent_id,owner_task_id,source_path,destination_path,staging_path,backup_path,requested_options_json,counters_json,phase,rollback_actions_json,error_json,lease_expires_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,NULL,?,?) ON CONFLICT(operation_id) DO UPDATE SET operation_type=excluded.operation_type, owner_agent_id=excluded.owner_agent_id, owner_task_id=excluded.owner_task_id, source_path=excluded.source_path, destination_path=excluded.destination_path, staging_path=excluded.staging_path, backup_path=excluded.backup_path, requested_options_json=excluded.requested_options_json, counters_json=excluded.counters_json, phase=excluded.phase, rollback_actions_json=excluded.rollback_actions_json, error_json=excluded.error_json, updated_at_ms=excluded.updated_at_ms")
            .bind(operation_id)
            .bind(operation_type)
            .bind(owner_agent_id)
            .bind(owner_task_id)
            .bind(source_path)
            .bind(destination_path)
            .bind(staging_path)
            .bind(backup_path)
            .bind(requested_options_json)
            .bind(counters_json)
            .bind(phase)
            .bind(rollback_actions_json)
            .bind(error_json)
            .bind(updated_at_ms)
            .bind(updated_at_ms)
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("upsert filesystem journal transition", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit filesystem journal transition", error))?;
        Ok(())
    }

    pub async fn list_filesystem_operation_journal_json(
        &self,
    ) -> Result<Vec<String>, StorageError> {
        let rows = sqlx::query(
            "SELECT operation_id,operation_type,owner_agent_id,owner_task_id,source_path,destination_path,staging_path,backup_path,requested_options_json,counters_json,phase,rollback_actions_json,error_json,updated_at_ms FROM filesystem_operation_journal ORDER BY updated_at_ms, operation_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| backend("list filesystem journals", error))?;
        rows.into_iter()
            .map(|row| {
                let parse_json = |column: &str| -> Result<serde_json::Value, StorageError> {
                    let raw: String = row
                        .try_get(column)
                        .map_err(|error| backend("read filesystem journal JSON column", error))?;
                    serde_json::from_str(&raw).map_err(|error| {
                        invalid_data(format!("invalid {column} in filesystem journal: {error}"))
                    })
                };
                let error_json = row
                    .try_get::<Option<String>, _>("error_json")
                    .map_err(|error| backend("read filesystem journal error", error))?
                    .map(|raw| serde_json::from_str::<serde_json::Value>(&raw))
                    .transpose()
                    .map_err(|error| {
                        invalid_data(format!("invalid filesystem journal error JSON: {error}"))
                    })?;
                let staging_path = row
                    .try_get::<Option<String>, _>("staging_path")
                    .map_err(|error| backend("read filesystem journal staging path", error))?
                    .unwrap_or_default();
                let backup_path = row
                    .try_get::<Option<String>, _>("backup_path")
                    .map_err(|error| backend("read filesystem journal backup path", error))?
                    .unwrap_or_default();
                let destination_path = row
                    .try_get::<Option<String>, _>("destination_path")
                    .map_err(|error| backend("read filesystem journal destination path", error))?
                    .unwrap_or_default();
                let value = serde_json::json!({
                    "operationId": row.try_get::<String, _>("operation_id").map_err(|error| backend("read filesystem journal operation id", error))?,
                    "operationType": row.try_get::<String, _>("operation_type").map_err(|error| backend("read filesystem journal operation type", error))?,
                    "ownerAgent": row.try_get::<String, _>("owner_agent_id").map_err(|error| backend("read filesystem journal owner", error))?,
                    "ownerTask": row.try_get::<Option<String>, _>("owner_task_id").map_err(|error| backend("read filesystem journal task", error))?,
                    "source": row.try_get::<String, _>("source_path").map_err(|error| backend("read filesystem journal source", error))?,
                    "destination": destination_path,
                    "stagingPath": staging_path,
                    "backupPath": backup_path,
                    "requestedOptions": parse_json("requested_options_json")?,
                    "phase": row.try_get::<String, _>("phase").map_err(|error| backend("read filesystem journal phase", error))?,
                    "counts": parse_json("counters_json")?,
                    "backupCreated": false,
                    "rollbackActions": parse_json("rollback_actions_json")?,
                    "warnings": [],
                    "error": error_json,
                    "updatedAtUnixMs": row.try_get::<i64, _>("updated_at_ms").map_err(|error| backend("read filesystem journal timestamp", error))?,
                });
                serde_json::to_string(&value).map_err(invalid_data)
            })
            .collect()
    }

    pub async fn remove_filesystem_operation_journal(
        &self,
        operation_id: &str,
    ) -> Result<(), StorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| backend("begin filesystem journal cleanup", error))?;
        sqlx::query("DELETE FROM filesystem_operation_journal WHERE operation_id=?")
            .bind(operation_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| backend("delete filesystem journal", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| backend("commit filesystem journal cleanup", error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SqliteRepository;

    #[tokio::test]
    async fn filesystem_journal_transitions_are_upserted_and_removed_transactionally() {
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("journal.sqlite3");
        let (repository, _) = SqliteRepository::open(&database, 2)
            .await
            .expect("open repository");
        let journal = |phase: &str| {
            serde_json::json!({
                "operationId": "op-1",
                "operationType": "copy",
                "ownerAgent": "agent",
                "ownerTask": "task",
                "source": directory.path().join("source"),
                "destination": directory.path().join("destination"),
                "stagingPath": directory.path().join("stage"),
                "backupPath": directory.path().join("backup"),
                "requestedOptions": {"verify": "metadata"},
                "phase": phase,
                "counts": {"files": 1, "directories": 0, "bytes": 5},
                "backupCreated": false,
                "rollbackActions": ["remove staging path"],
                "warnings": [],
                "error": null,
                "updatedAtUnixMs": 1234,
            })
            .to_string()
        };

        repository
            .upsert_filesystem_operation_journal_json(&journal("staging"))
            .await
            .expect("insert journal");
        let phase: String = sqlx::query_scalar(
            "SELECT phase FROM filesystem_operation_journal WHERE operation_id='op-1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("read phase");
        assert_eq!(phase, "staging");

        repository
            .upsert_filesystem_operation_journal_json(&journal("verifying"))
            .await
            .expect("update journal");
        let phase: String = sqlx::query_scalar(
            "SELECT phase FROM filesystem_operation_journal WHERE operation_id='op-1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("read updated phase");
        assert_eq!(phase, "verifying");
        let active = repository
            .list_filesystem_operation_journal_json()
            .await
            .expect("list active journals");
        assert_eq!(active.len(), 1);
        let active_value: serde_json::Value =
            serde_json::from_str(&active[0]).expect("active journal JSON");
        assert_eq!(active_value["operationId"], "op-1");
        assert_eq!(active_value["phase"], "verifying");

        repository
            .remove_filesystem_operation_journal("op-1")
            .await
            .expect("remove journal");
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM filesystem_operation_journal WHERE operation_id='op-1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("count journal rows");
        assert_eq!(remaining, 0);
    }
}
