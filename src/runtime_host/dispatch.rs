use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use chatcmd_core::{
    ArtifactId, ArtifactStore, ExecutionMode, TaskExecutionMode, TaskId, TaskStore,
};
use chatcmd_mcp::RuntimeApi as _;
use chatcmd_runtime::{
    OperationContext, RuntimeError, RuntimeResult, SearchProgress, ShellCreateRequest,
    ShellWriteRequest,
};
use serde_json::{Value, json};

use super::inputs::*;
use super::{RuntimeHost, invalid, now_ms, parse, storage_error, task_json, value};

impl RuntimeHost {
    pub(super) async fn authorize_tool(&self, agent_id: &str, tool: &str) -> RuntimeResult<()> {
        use chatcmd_core::{McpAgentStore as _, ToolCatalogStore as _};

        let id = chatcmd_core::AgentId::new(agent_id)
            .map_err(|_| invalid("agentId", "must be a non-empty string"))?;
        self.repository
            .agent(&id)
            .await
            .map_err(storage_error)?
            .filter(|agent| agent.enabled)
            .ok_or_else(|| RuntimeError::new("unauthorized", "agent is disabled or missing"))?;
        if matches!(
            tool,
            "agent_user_message"
                | "agent_progress"
                | "agent_subagent_start"
                | "agent_subagent_wait"
                | "agent_turn_complete"
        ) {
            return Ok(());
        }
        let allowed = self
            .repository
            .agent_allowed_tool_ids(&id)
            .await
            .map_err(storage_error)?;
        let tools = self.repository.list_tools().await.map_err(storage_error)?;
        if tools.iter().any(|candidate| {
            candidate.key == tool && candidate.enabled && allowed.contains(&candidate.id)
        }) {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "policy_denied",
                "agent tool allowlist denied this operation",
            ))
        }
    }

    pub(super) async fn dispatch(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let user_path_scopes =
            if tool.starts_with("fs_") || tool.starts_with("git_") || tool == "shell_create" {
                self.task_user_path_scopes(&context).await?
            } else {
                Vec::new()
            };
        let scoped_workspace = if tool.starts_with("fs_") || tool.starts_with("git_") {
            Some(self.workspace.with_additional_scopes(&user_path_scopes)?)
        } else {
            None
        };
        let workspace = scoped_workspace.as_ref().unwrap_or(&self.workspace);
        let scoped_git = scoped_workspace
            .clone()
            .map(|workspace| self.git.with_workspace(workspace));
        let git = scoped_git.as_ref().unwrap_or(&self.git);

        match tool {
            "device_list" => value(vec![self.local_device()]),
            "device_get" => {
                let input: DeviceGet = parse(arguments)?;
                if input.device_id != self.device.id.as_str() && input.device_id != "local" {
                    return Err(RuntimeError::new(
                        "device_not_found",
                        "device was not found",
                    ));
                }
                value(self.local_device())
            }
            "shell_create" => {
                let input: ShellCreate = parse(arguments)?;
                let info = self
                    .shell
                    .create_with_additional_scopes(
                        &context,
                        ShellCreateRequest {
                            request_id: context.request_id.clone(),
                            working_directory: input.working_directory,
                            executable: input.executable,
                            arguments: input.arguments,
                            environment: input.environment,
                            columns: input.columns,
                            rows: input.rows,
                        },
                        &user_path_scopes,
                    )
                    .await?;
                self.persist_shell_session(&context, &info).await?;
                value(info)
            }
            "shell_write" => {
                let input: ShellWrite = parse(arguments)?;
                let written = self
                    .shell
                    .write(
                        &context,
                        ShellWriteRequest {
                            request_id: context.request_id.clone(),
                            session_id: input.session_id,
                            text: input.text,
                            append_new_line: input.append_new_line,
                        },
                    )
                    .await?;
                Ok(json!({ "writtenBytes": written }))
            }
            "shell_wait" => {
                let input: ShellWait = parse(arguments)?;
                let result = self
                    .shell
                    .wait(
                        &input.session_id,
                        Duration::from_millis(input.timeout_ms.clamp(1, 300_000)),
                    )
                    .await?;
                if result.completed {
                    self.update_session_status(&input.session_id, "exited", result.exit_code)
                        .await?;
                }
                value(result)
            }
            "shell_read" => {
                let input: ShellRead = parse(arguments)?;
                let result = self
                    .shell
                    .read(
                        &input.session_id,
                        input.after_sequence,
                        input.max_events.clamp(1, 2_000),
                    )
                    .await?;
                self.persist_shell_events(&context, &result).await?;
                value(result)
            }
            "shell_signal" => {
                let input: ShellSignalInput = parse(arguments)?;
                self.shell
                    .signal(&context, &input.session_id, input.signal)
                    .await?;
                Ok(json!({ "accepted": true }))
            }
            "shell_resize" => {
                let input: ShellResize = parse(arguments)?;
                value(
                    self.shell
                        .resize(&input.session_id, input.columns, input.rows)
                        .await?,
                )
            }
            "shell_close" => {
                let input: ShellClose = parse(arguments)?;
                self.shell
                    .close(&context, &input.session_id, input.force)
                    .await?;
                self.update_session_status(&input.session_id, "closed", None)
                    .await?;
                Ok(json!({ "closed": true }))
            }
            "shell_list" => value(self.shell.list().await?),
            "shell_inspect" => {
                let input: SessionInput = parse(arguments)?;
                value(self.shell.inspect(&input.session_id).await?)
            }
            "workspace_roots" => value(self.workspace.roots()),
            "fs_list" => {
                let input: ListInput = parse(arguments)?;
                value(
                    workspace
                        .list(&input.path, input.offset, input.limit)
                        .await?,
                )
            }
            "fs_search" => {
                let input: SearchInput = parse(arguments)?;
                let host = self.clone();
                let progress_context = context.clone();
                let progress_sequence = Arc::new(AtomicU64::new(0));
                value(
                    workspace
                        .search(
                            &input.path,
                            &input.query,
                            input.case_sensitive,
                            input.max_results,
                            input.max_file_bytes,
                            move |progress: SearchProgress| {
                                let sequence =
                                    progress_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                                let text = if let Some(matched) = progress.matched {
                                    format!("MATCH {}\n", matched)
                                } else {
                                    format!(
                                        "Scanning {} files · {} matches\n{}\n",
                                        progress.files_scanned,
                                        progress.matches_found,
                                        progress.path.display()
                                    )
                                };
                                host.publish_event(
                                    format!(
                                        "{}:search-progress:{sequence}",
                                        progress_context.request_id
                                    ),
                                    "terminal_output",
                                    progress_context.task_id.clone(),
                                    progress_context.mcp_session_id.clone(),
                                    progress_context.turn_id.clone(),
                                    json!({
                                        "text": text,
                                        "stream": "tool",
                                        "encoding": "utf-8",
                                        "activityId": progress_context.request_id
                                    }),
                                );
                            },
                        )
                        .await?,
                )
            }
            "fs_find" => {
                let input: FindInput = parse(arguments)?;
                value(
                    workspace
                        .find(
                            &input.path,
                            &input.pattern,
                            input.max_results,
                            input.max_depth,
                        )
                        .await?,
                )
            }
            "fs_read_text" => {
                let input: ReadInput = parse(arguments)?;
                value(
                    workspace
                        .read_text_range(
                            &input.path,
                            input.max_characters,
                            input.start_line,
                            input.line_count,
                        )
                        .await?,
                )
            }
            "fs_write_text" => {
                let input: WriteTextInput = parse(arguments)?;
                value(
                    workspace
                        .write_text(&context, &input.path, &input.content, input.overwrite)
                        .await?,
                )
            }
            "fs_replace_text" => {
                let input: ReplaceTextInput = parse(arguments)?;
                value(
                    workspace
                        .replace_text(
                            &context,
                            &input.path,
                            &input.old_text,
                            &input.new_text,
                            input.expected_occurrences,
                        )
                        .await?,
                )
            }
            "fs_write_raw" => {
                let input: WriteRawInput = parse(arguments)?;
                value(
                    workspace
                        .write_raw(&context, &input.path, &input.base64, input.overwrite)
                        .await?,
                )
            }
            "fs_stat" => {
                let input: PathInput = parse(arguments)?;
                value(workspace.stat(&input.path).await?)
            }
            "fs_create_directory" => {
                let input: PathInput = parse(arguments)?;
                value(workspace.create_directory(&input.path).await?)
            }
            "fs_copy" => {
                let input: TransferInput = parse(arguments)?;
                value(
                    workspace
                        .copy(&context, &input.source, &input.destination, input.overwrite)
                        .await?,
                )
            }
            "fs_move" => {
                let input: TransferInput = parse(arguments)?;
                value(
                    workspace
                        .move_path(&context, &input.source, &input.destination, input.overwrite)
                        .await?,
                )
            }
            "fs_delete" => {
                let input: DeleteInput = parse(arguments)?;
                Ok(json!({
                    "deleted": workspace.delete(&context, &input.path, input.recursive).await?
                }))
            }
            "git_status" => {
                let input: CwdInput = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(git.status(&cwd).await?)
            }
            "git_diff" => {
                let input: GitDiff = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(
                    git.diff(&cwd, input.staged, input.stat, input.path.as_deref())
                        .await?,
                )
            }
            "git_log" => {
                let input: GitLog = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(git.log(&cwd, input.count, input.path.as_deref()).await?)
            }
            "git_branch" => {
                let input: CwdInput = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(git.branch(&cwd).await?)
            }
            "git_show" => {
                let input: GitShow = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(
                    git.show(&cwd, &input.revision, input.path.as_deref())
                        .await?,
                )
            }
            "git_commit" => {
                let input: GitCommit = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                value(
                    git.commit(&cwd, &input.message, input.all, &input.paths)
                        .await?,
                )
            }
            "process_list" => value(self.process.list().await?),
            "process_inspect" => {
                let input: ProcessInput = parse(arguments)?;
                value(self.process.inspect(input.process_id).await?)
            }
            "process_kill" => {
                let input: ProcessKill = parse(arguments)?;
                self.process
                    .kill(&context, input.process_id, input.entire_tree)
                    .await?;
                Ok(json!({ "killed": true }))
            }
            "skills_list" => value(self.skills.list().await?),
            "skill_read" => {
                let input: SkillInput = parse(arguments)?;
                value(self.skills.read(&input.skill_id).await?)
            }
            "task_get" => {
                let id = context_task_id(&context)?;
                Ok(self
                    .repository
                    .task(&id)
                    .await
                    .map_err(storage_error)?
                    .map_or(Value::Null, task_json))
            }
            "task_list" => Ok(Value::Array(
                self.repository
                    .list_tasks(200)
                    .await
                    .map_err(storage_error)?
                    .into_iter()
                    .map(task_json)
                    .collect(),
            )),
            "task_set_execution_mode" => {
                let input: ExecutionModeInput = parse(arguments)?;
                let id = context_task_id(&context)?;
                let mode = match input.mode.as_str() {
                    "approval" => ExecutionMode::Approval,
                    "allow" | "safe" | "unrestricted" => ExecutionMode::Allow,
                    "deny" => ExecutionMode::Deny,
                    _ => return Err(invalid("mode", "must be approval, allow, or deny")),
                };
                self.repository
                    .set_execution_mode(&TaskExecutionMode {
                        task_id: id,
                        mode,
                        updated_at_ms: now_ms(),
                    })
                    .await
                    .map_err(storage_error)?;
                Ok(json!({ "mode": mode.as_str() }))
            }
            "task_artifact_list" => self.list_artifacts(&context).await,
            "task_artifact_read" => self.read_artifact(&context, arguments).await,
            "agent_user_message" => {
                let input: UserMessageInput = parse(arguments)?;
                self.save_user_message(&context, &input.content).await
            }
            "agent_progress" => {
                let input: ProgressInput = parse(arguments)?;
                self.save_agent_event(
                    &context,
                    "progress",
                    &input.message,
                    input.suggested_title.as_deref(),
                )
                .await
            }
            "agent_subagent_start" => {
                let input: SubagentStartInput = parse(arguments)?;
                self.register_subagent(&context, &input.name, &input.request)
                    .await
            }
            "agent_subagent_wait" => {
                let input: SubagentWaitInput = parse(arguments)?;
                self.wait_for_subagents(&context, input.timeout_ms).await
            }
            "agent_turn_complete" => {
                let input: CompleteInput = parse(arguments)?;
                self.ensure_subagents_finished(&context).await?;
                self.save_agent_event(
                    &context,
                    "completed",
                    &input.content,
                    input.suggested_title.as_deref(),
                )
                .await
            }
            _ => Err(RuntimeError::new("tool_not_found", "unknown MCP tool")),
        }
    }

    async fn list_artifacts(&self, context: &OperationContext) -> RuntimeResult<Value> {
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
                    json!({
                        "id": row.get::<String, _>("id"),
                        "relativePath": row.get::<String, _>("relative_path"),
                        "mediaType": row.get::<Option<String>, _>("media_type"),
                        "sizeBytes": row.get::<i64, _>("size_bytes"),
                        "createdAtMs": row.get::<i64, _>("created_at_ms")
                    })
                })
                .collect(),
        ))
    }

    async fn read_artifact(
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
                "relativePath": artifact.relative_path, "mediaType": artifact.media_type,
                "sizeBytes": artifact.size_bytes, "sha256Hex": artifact.sha256_hex
            },
            "content": read.content, "truncated": read.truncated
        }))
    }
}

fn context_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}
