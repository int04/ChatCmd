use std::{
    path::{Component, Path},
    time::{Duration, Instant},
};

mod artifact_tools;
mod command_tools;
mod filesystem_tools;
mod helpers;
mod tool_authorization;

use chatcmd_core::{
    Artifact, ArtifactId, ArtifactStore, ExecutionMode, TaskExecutionMode, TaskId, TaskStore,
};
use chatcmd_mcp::RuntimeApi as _;
use chatcmd_runtime::{
    CommandOutput, FsConflictPolicy, FsTransferRequest, OperationContext, RuntimeError,
    RuntimeResult, ShellCreateRequest, ShellWriteRequest, ToolUsage,
};
use serde_json::{Value, json};

use super::turn_file_changes::{FileChangeKind, capture_snapshot};
use super::{
    RuntimeHost, filesystem_dispatch, inputs::*, invalid, now_ms, parse, storage_error, task_json,
    value,
};

impl RuntimeHost {
    pub(super) async fn dispatch(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let project_folder = if tool.starts_with("fs_")
            || tool.starts_with("git_")
            || tool == "shell_create"
            || tool == "command_run"
            || tool == "workspace_roots"
            || tool == "project_context"
            || matches!(tool, "skills_list" | "skill_read")
        {
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await?
                .map(std::path::PathBuf::from)
        } else {
            None
        };
        let mut task_path_scopes = if tool.starts_with("fs_")
            || tool.starts_with("git_")
            || matches!(tool, "command_run" | "shell_create" | "workspace_roots")
        {
            self.task_user_path_scopes(&context).await?
        } else {
            Vec::new()
        };
        if let Some(project_folder) = project_folder.as_ref().filter(|path| path.is_dir())
            && !task_path_scopes.contains(project_folder)
        {
            task_path_scopes.push(project_folder.clone());
        }
        let scoped_workspace = if tool.starts_with("fs_") || tool.starts_with("git_") {
            Some(self.workspace.with_additional_scopes(&task_path_scopes)?)
        } else {
            None
        };
        let workspace = scoped_workspace.as_ref().unwrap_or(&self.workspace);
        let arguments = if tool.starts_with("fs_") {
            filesystem_dispatch::resolve_relative_paths(arguments, project_folder.as_deref())?
        } else {
            arguments
        };
        let scoped_git = scoped_workspace
            .clone()
            .map(|workspace| self.git.with_workspace(workspace));
        let git = scoped_git.as_ref().unwrap_or(&self.git);

        if is_filesystem_tool(tool) {
            return self
                .dispatch_filesystem_tool(tool, &context, arguments, workspace)
                .await;
        }

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
                self.retire_idle_current_turn_terminals(&context).await?;
                let working_directory = match input.working_directory {
                    Some(value) if value.is_absolute() => value,
                    Some(value) => project_folder
                        .as_ref()
                        .map(|folder| folder.join(value))
                        .ok_or_else(project_folder_required_for_shell)?,
                    None => project_folder
                        .clone()
                        .ok_or_else(project_folder_required_for_shell)?,
                };
                self.enable_shell_file_watcher(&context);
                let info = self
                    .shell
                    .create_with_additional_scopes(
                        &context,
                        ShellCreateRequest {
                            request_id: context.request_id.clone(),
                            working_directory: Some(working_directory),
                            executable: input.executable,
                            arguments: input.arguments,
                            environment: input.environment,
                            columns: input.columns,
                            rows: input.rows,
                        },
                        &task_path_scopes,
                    )
                    .await?;
                self.persist_shell_session(&context, &info).await?;
                self.publish_terminal_opened(&context, &info);
                self.spawn_terminal_live_bridge(&context, &info);
                value(info)
            }
            "command_run" => {
                self.dispatch_command_run(
                    &context,
                    arguments,
                    project_folder.as_deref(),
                    &task_path_scopes,
                )
                .await
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
                            input_kind: input.input_kind,
                            sensitive: input.sensitive,
                        },
                    )
                    .await?;
                Ok(json!({ "writtenBytes": written }))
            }
            "shell_wait" => {
                let input: ShellWait = parse(arguments)?;
                let (result, usage) = self
                    .shell
                    .wait_with_context(
                        &context,
                        &input.session_id,
                        Duration::from_millis(input.timeout_ms.clamp(1, 300_000)),
                    )
                    .await?;
                if result.completed {
                    self.update_session_status(&input.session_id, "exited", result.exit_code)
                        .await?;
                }
                value_with_usage(result, usage)
            }
            "shell_read" => {
                let input: ShellRead = parse(arguments)?;
                let (result, usage) = self
                    .shell
                    .read_with_context(
                        &context,
                        &input.session_id,
                        input.after_sequence,
                        input.max_events.clamp(1, 2_000),
                    )
                    .await?;
                self.persist_shell_events(&context, &result).await?;
                value_with_usage(result, usage)
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
                        .resize_with_context(&context, &input.session_id, input.columns, input.rows)
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
            "workspace_roots" => match project_folder {
                Some(project_folder) => value(vec![project_folder]),
                None => value(task_path_scopes),
            },
            "project_context" => {
                let input: ProjectContextInput = parse(arguments)?;
                let folder = project_folder.ok_or_else(|| {
                    RuntimeError::new(
                        "project_folder_required",
                        "project context requires the current task project folder",
                    )
                })?;
                value(
                    chatcmd_runtime::ProjectContextService::default()
                        .load_with_options(&folder, &input.target_paths, input.policy, input.range)
                        .await?,
                )
            }
            "blob_begin" => value(self.blob_store.begin(
                &context,
                parse::<chatcmd_runtime::BlobBeginRequest>(arguments)?,
            )?),
            "blob_write_chunk" => value(self.blob_store.write_chunk(
                &context,
                parse::<chatcmd_runtime::BlobChunkRequest>(arguments)?,
            )?),
            "blob_status" => {
                let input: BlobStatusInput = parse(arguments)?;
                value(self.blob_store.status_with_budget(
                    &context,
                    &input.upload_id,
                    &input.budget,
                )?)
            }
            "blob_seal" => value(self.blob_store.seal(
                &context,
                parse::<chatcmd_runtime::BlobSealRequest>(arguments)?,
            )?),
            "blob_abort" => {
                let input: BlobStatusInput = parse(arguments)?;
                value(self.blob_store.abort_with_budget(
                    &context,
                    &input.upload_id,
                    &input.budget,
                )?)
            }
            "git_status" => {
                let input: GitCwdInput = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                let output = git
                    .status_with_options(&cwd, &input.options, context.cancellation.clone())
                    .await?;
                value(self.register_git_artifact(&context, output).await?)
            }
            "git_diff" => {
                let input: GitDiff = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                let output = git
                    .diff_with_options(
                        &cwd,
                        input.staged,
                        input.stat,
                        input.path.as_deref(),
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?;
                value(self.register_git_artifact(&context, output).await?)
            }
            "git_log" => {
                let input: GitLog = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                let output = git
                    .log_with_options(
                        &cwd,
                        input.count,
                        input.path.as_deref(),
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?;
                value(self.register_git_artifact(&context, output).await?)
            }
            "git_branch" => {
                let input: GitCwdInput = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                let output = git
                    .branch_with_options(&cwd, &input.options, context.cancellation.clone())
                    .await?;
                value(self.register_git_artifact(&context, output).await?)
            }
            "git_show" => {
                let input: GitShow = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                let output = git
                    .show_with_options(
                        &cwd,
                        &input.revision,
                        input.path.as_deref(),
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?;
                value(self.register_git_artifact(&context, output).await?)
            }
            "git_commit" => {
                let input: GitCommit = parse(arguments)?;
                let cwd = self.resolve_git_cwd(&context, input.cwd).await?;
                if input.preview_only {
                    return value(
                        git.preview_commit_with_options(
                            &cwd,
                            input.all,
                            &input.paths,
                            &input.options,
                            context.cancellation.clone(),
                        )
                        .await?,
                    );
                }
                let output = if let Some(preview) = input.expected_preview.as_ref() {
                    git.commit_previewed_with_options(
                        &cwd,
                        &input.message,
                        input.all,
                        &input.paths,
                        preview,
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?
                } else {
                    git.commit_with_options(
                        &cwd,
                        &input.message,
                        input.all,
                        &input.paths,
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?
                };
                value(self.register_git_artifact(&context, output).await?)
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
            "skills_list" => value(
                self.skills
                    .list_for_workspace(project_folder.as_deref())
                    .await?,
            ),
            "skill_read" => {
                let input: SkillInput = parse(arguments)?;
                value(
                    self.skills
                        .read_for_workspace(&input.skill_id, project_folder.as_deref())
                        .await?,
                )
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
            "task_artifact_create" => self.create_artifact(&context, arguments).await,
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
            "agent_plan_question" => {
                let input: PlanQuestionInput = parse(arguments)?;
                self.ask_plan_question_with_kind(
                    &context,
                    input.question,
                    input.options,
                    input.question_kind,
                )
                .await
            }
            "agent_subagent_start" => {
                let input: SubagentStartInput = parse(arguments)?;
                super::subagent_contract::validate_delegation_contract(&input)?;
                self.register_subagent(
                    &context,
                    &input.name,
                    &input.request,
                    input.approval_grant.as_ref(),
                )
                .await
            }
            "agent_subagent_wait" => {
                let input: SubagentWaitInput = parse(arguments)?;
                self.wait_for_subagents(&context, input.timeout_ms).await
            }
            "agent_turn_complete" => self.complete_agent_turn(&context, arguments).await,
            _ => Err(RuntimeError::new("tool_not_found", "unknown MCP tool")),
        }
    }
}

use helpers::*;
