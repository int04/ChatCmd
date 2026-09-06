use super::*;

impl RuntimeHost {
    pub(super) async fn register_git_artifact(
        &self,
        context: &OperationContext,
        mut output: CommandOutput,
    ) -> RuntimeResult<CommandOutput> {
        let Some(path) = output.artifact_ref.clone() else {
            return Ok(output);
        };
        let source = std::path::PathBuf::from(&path);
        let managed = self.blob_store.store_artifact_file(
            context,
            &source,
            Some("text/plain; charset=utf-8".to_owned()),
            24 * 60 * 60,
        )?;
        let artifact_id = ArtifactId::new(format!("artifact-{}", uuid::Uuid::new_v4()))
            .map_err(|error| invalid("artifactId", error))?;
        let timestamp = now_ms();
        let artifact = Artifact {
            id: artifact_id.clone(),
            task_id: context_task_id(context)?,
            session_id: None,
            relative_path: format!(
                "{}{}",
                super::super::MANAGED_ARTIFACT_PREFIX,
                managed.content_ref
            ),
            media_type: Some("text/plain; charset=utf-8".to_owned()),
            size_bytes: i64::try_from(managed.size_bytes).map_err(|_| {
                RuntimeError::new("artifactTooLarge", "artifact size cannot be represented")
            })?,
            sha256_hex: Some(managed.sha256.clone()),
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        self.repository
            .register_artifact(&artifact)
            .await
            .map_err(storage_error)?;
        let _ = tokio::fs::remove_file(&source).await;
        self.telemetry.set_blob_bytes(self.blob_store.usage_bytes());
        output.artifact_ref = Some(artifact_id.into_string());
        output.artifact_sha256 = Some(managed.sha256);
        Ok(output)
    }

    pub(super) async fn create_artifact(
        &self,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let task_id = context_task_id(context)?;
        let input: ArtifactCreateInput = parse(arguments)?;
        let relative = Path::new(&input.relative_path);
        if input.relative_path.trim().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(RuntimeError::new(
                "invalid_arguments",
                "relativePath must be a non-empty workspace-relative path without '.' or '..' components",
            ));
        }
        let root = self.workspace.roots().first().ok_or_else(|| {
            RuntimeError::new("workspace_unavailable", "no workspace root is configured")
        })?;
        let target = root.join(relative);
        let target_existed = target.exists();
        let before = capture_snapshot(&target);
        let lease = self
            .blob_store
            .lease(context, &input.content_ref, "artifact")?;
        let size_bytes = i64::try_from(lease.size_bytes).map_err(|_| {
            RuntimeError::new("artifactTooLarge", "artifact size cannot be represented")
        })?;
        let sha256_hex = lease.sha256.clone();
        let write_result = self
            .workspace
            .write_blob_atomic(
                context,
                &target,
                lease.path(),
                chatcmd_runtime::AtomicWriteOptions::default(),
                false,
            )
            .await;
        if let Err(error) = write_result {
            lease.finish(false)?;
            return Err(error);
        }

        let artifact_id = ArtifactId::new(format!("artifact-{}", uuid::Uuid::new_v4()))
            .map_err(|error| invalid("artifactId", error))?;
        let timestamp = now_ms();
        let artifact = Artifact {
            id: artifact_id.clone(),
            task_id: task_id.clone(),
            session_id: None,
            relative_path: input.relative_path.clone(),
            media_type: input.media_type.clone(),
            size_bytes,
            sha256_hex: Some(sha256_hex.clone()),
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        if let Err(error) = self.repository.register_artifact(&artifact).await {
            let _ = tokio::fs::remove_file(&target).await;
            lease.finish(false)?;
            return Err(storage_error(error));
        }
        lease.finish(true)?;
        self.record_committed_change(
            context,
            &target,
            None,
            if target_existed {
                FileChangeKind::Modified
            } else {
                FileChangeKind::Added
            },
            before,
            capture_snapshot(&target),
            None,
            Some(artifact_id.as_str().to_owned()),
        );
        Ok(json!({
            "artifactId": artifact_id.as_str(),
            "taskId": task_id.as_str(),
            "relativePath": input.relative_path,
            "mediaType": input.media_type,
            "sizeBytes": size_bytes,
            "sha256Hex": sha256_hex
        }))
    }

    pub(super) async fn list_artifacts(&self, context: &OperationContext) -> RuntimeResult<Value> {
        use sqlx::Row as _;

        let task_id = context_task_id(context)?;
        let rows = sqlx::query("SELECT id,relative_path,media_type,size_bytes,created_at_ms FROM artifact_registry WHERE task_id=? ORDER BY created_at_ms,id")
            .bind(task_id.as_str())
            .fetch_all(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "artifact list unavailable"))?;
        Ok(Value::Array(
            rows.iter()
                .map(|row| {
                    let relative_path = row.get::<String, _>("relative_path");
                    let managed = relative_path.starts_with(super::super::MANAGED_ARTIFACT_PREFIX);
                    json!({
                        "id": row.get::<String, _>("id"),
                        "relativePath": (!managed).then_some(relative_path),
                        "managed": managed,
                        "mediaType": row.get::<Option<String>, _>("media_type"),
                        "sizeBytes": row.get::<i64, _>("size_bytes"),
                        "createdAtMs": row.get::<i64, _>("created_at_ms")
                    })
                })
                .collect(),
        ))
    }

    pub(super) async fn read_artifact(
        &self,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let task_id = context_task_id(context)?;
        let input: ArtifactInput = parse(arguments)?;
        let id =
            ArtifactId::new(input.artifact_id).map_err(|error| invalid("artifactId", error))?;
        let artifact = self
            .repository
            .artifact(&id)
            .await
            .map_err(storage_error)?
            .filter(|artifact| artifact.task_id.as_str() == task_id.as_str())
            .ok_or_else(|| RuntimeError::new("artifact_not_found", "artifact was not found"))?;
        if let Some(content_ref) = artifact
            .relative_path
            .strip_prefix(super::super::MANAGED_ARTIFACT_PREFIX)
        {
            let read = self.blob_store.read_artifact_text_range(
                context,
                content_ref,
                input.offset,
                input.max_bytes.clamp(1, 256 * 1024),
            )?;
            return Ok(json!({
                "artifact": {
                    "id": artifact.id.as_str(), "taskId": artifact.task_id.as_str(),
                    "sessionId": artifact.session_id.map(|id| id.into_string()),
                    "relativePath": Value::Null, "managed": true, "mediaType": artifact.media_type,
                    "sizeBytes": artifact.size_bytes, "sha256Hex": artifact.sha256_hex,
                    "expiresAtMs": read.expires_at_ms
                },
                "content": read.content,
                "truncated": read.truncated,
                "offset": read.offset,
                "nextOffset": read.next_offset,
                "hasMore": read.next_offset.is_some()
            }));
        }
        let path = self
            .workspace
            .roots()
            .iter()
            .map(|root| root.join(&artifact.relative_path))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                RuntimeError::new("artifact_not_found", "artifact file was not found")
            })?;
        let read = self.workspace.read_text(&path, 200_000).await?;
        Ok(json!({
            "artifact": {
                "id": artifact.id.as_str(), "taskId": artifact.task_id.as_str(),
                "sessionId": artifact.session_id.map(|id| id.into_string()),
                "relativePath": artifact.relative_path, "managed": false, "mediaType": artifact.media_type,
                "sizeBytes": artifact.size_bytes, "sha256Hex": artifact.sha256_hex
            },
            "content": read.content, "truncated": read.truncated
        }))
    }
}
