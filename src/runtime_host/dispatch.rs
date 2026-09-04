use std::{
    path::{Component, Path},
    time::{Duration, Instant},
};

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
                | "agent_plan_question"
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
        let project_folder = if tool.starts_with("fs_")
            || tool.starts_with("git_")
            || tool == "shell_create"
            || tool == "workspace_roots"
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
            || matches!(tool, "shell_create" | "workspace_roots")
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
            "fs_list" => {
                let input: ListInput = parse(arguments)?;
                value(
                    workspace
                        .list(&input.path, input.offset, input.limit.clamp(1, 2_000))
                        .await?,
                )
            }
            "fs_list_v2" => {
                let started = Instant::now();
                let input: ListV2Input = parse(arguments)?;
                let scope = workspace.stat(&input.path).await?.path;
                let normalized_scope = scope.to_string_lossy();
                let cursor_state = input
                    .cursor
                    .as_deref()
                    .map(|cursor| {
                        self.cursor_codec
                            .decode::<chatcmd_runtime::FsListCursorState>(
                                cursor,
                                "fs_list_v2",
                                normalized_scope.as_ref(),
                            )
                    })
                    .transpose()?;
                let request = chatcmd_runtime::FsListRequestV2 {
                    path: input.path,
                    limit: input.limit.clamp(1, 2_000),
                    sort: input.sort,
                    metadata: input.metadata,
                    include_hidden: input.include_hidden,
                    budget: input.budget,
                };
                let (page, state_id) = workspace
                    .list_v2(
                        &context,
                        &request,
                        cursor_state.as_ref().map(|state| state.state_id.as_str()),
                        cursor_state
                            .as_ref()
                            .map(|state| state.directory_version.as_str()),
                    )
                    .await?;
                let next_cursor = match (page.has_more, state_id) {
                    (true, Some(state_id)) => Some(self.cursor_codec.encode(
                        "fs_list_v2",
                        normalized_scope.as_ref(),
                        &chatcmd_runtime::FsListCursorState {
                            state_id,
                            directory_version: page.data.directory_version.clone(),
                        },
                        None,
                    )?),
                    _ => None,
                };
                let returned_items = u64::try_from(page.data.items.len()).unwrap_or(u64::MAX);
                let mut result = chatcmd_runtime::ToolResultEnvelope::paged(
                    page.data,
                    next_cursor,
                    page.has_more,
                );
                if let Some(reason) = page.truncation_reason {
                    result.truncation = Some(chatcmd_runtime::TruncationInfo {
                        truncated: true,
                        reason: Some(reason),
                        returned_items,
                        omitted_items: None,
                    });
                }
                result.warnings = page.warnings;
                result = result.with_usage(chatcmd_runtime::ToolUsage {
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    entries_scanned: Some(page.entries_scanned),
                    metadata_calls: Some(page.metadata_calls),
                    ..chatcmd_runtime::ToolUsage::default()
                });
                result.measure_output_bytes()?;
                value(result)
            }
            "fs_search" => {
                filesystem_dispatch::search(self, workspace, &context, parse(arguments)?).await
            }
            "fs_find" => {
                let started = Instant::now();
                let input: FindInput = parse(arguments)?;
                let scope = workspace.stat(&input.path).await?.path;
                let normalized_scope = scope.to_string_lossy();
                let cursor_state = input
                    .cursor
                    .as_deref()
                    .map(|cursor| {
                        self.cursor_codec
                            .decode::<chatcmd_runtime::FsFindCursorState>(
                                cursor,
                                "fs_find",
                                normalized_scope.as_ref(),
                            )
                    })
                    .transpose()?;
                let legacy_pattern = input.pattern_mode.is_none();
                let pattern = if legacy_pattern {
                    input.pattern.trim_matches('*').to_owned()
                } else {
                    input.pattern
                };
                let request = chatcmd_runtime::FsFindRequest {
                    path: input.path,
                    pattern,
                    pattern_mode: input
                        .pattern_mode
                        .unwrap_or(chatcmd_runtime::FindPatternMode::Literal),
                    case_sensitive: input.case_sensitive,
                    entry_types: input.entry_types,
                    max_depth: input.max_depth.clamp(1, 128),
                    include_ignored: input.include_ignored,
                    include_hidden: input.include_hidden,
                    exclude: input.exclude,
                    extensions: input.extensions,
                    limit: input
                        .limit
                        .or(input.max_results)
                        .unwrap_or(200)
                        .clamp(1, 5_000),
                    budget: input.budget,
                };
                let (page, state_id) = workspace
                    .find_v2(
                        &context,
                        &request,
                        cursor_state.as_ref().map(|state| state.state_id.as_str()),
                        cursor_state
                            .as_ref()
                            .map(|state| state.root_version.as_str()),
                    )
                    .await?;
                let next_cursor = match (page.has_more, state_id) {
                    (true, Some(state_id)) => Some(self.cursor_codec.encode(
                        "fs_find",
                        normalized_scope.as_ref(),
                        &chatcmd_runtime::FsFindCursorState {
                            state_id,
                            root_version: page.root_version.clone(),
                        },
                        None,
                    )?),
                    _ => None,
                };
                let returned_items = u64::try_from(page.data.items.len()).unwrap_or(u64::MAX);
                let mut result = chatcmd_runtime::ToolResultEnvelope::paged(
                    page.data,
                    next_cursor,
                    page.has_more,
                );
                if let Some(reason) = page.truncation_reason {
                    result.truncation = Some(chatcmd_runtime::TruncationInfo {
                        truncated: true,
                        reason: Some(reason),
                        returned_items,
                        omitted_items: None,
                    });
                }
                result.warnings = page.warnings;
                if legacy_pattern {
                    result.warnings.push(chatcmd_runtime::ToolWarning {
                        code: "legacy_find_pattern".to_owned(),
                        message: "patternMode omitted; using legacy case-insensitive literal-contains semantics after trimming outer '*'".to_owned(),
                    });
                }
                result = result.with_usage(chatcmd_runtime::ToolUsage {
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    entries_scanned: Some(page.entries_scanned),
                    metadata_calls: Some(page.metadata_calls),
                    ..chatcmd_runtime::ToolUsage::default()
                });
                result.measure_output_bytes()?;
                value(result)
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
            "fs_read_text_v2" => {
                let input: chatcmd_runtime::TextReadRequestV2 = parse(arguments)?;
                value(workspace.read_text_v2(Some(&context), &input).await?)
            }
            "fs_batch_read" => {
                let input: chatcmd_runtime::FsBatchReadRequest = parse(arguments)?;
                value(workspace.batch_read(&context, &input).await?)
            }
            "fs_write_text" => {
                filesystem_dispatch::write_text(self, workspace, &context, parse(arguments)?).await
            }
            "fs_replace_text" => {
                filesystem_dispatch::replace_text(self, workspace, &context, parse(arguments)?)
                    .await
            }
            "fs_apply_edits" => {
                filesystem_dispatch::apply_edits(self, workspace, &context, parse(arguments)?).await
            }
            "fs_write_raw" => {
                let input: WriteRawInput = parse(arguments)?;
                filesystem_dispatch::write_raw(self, workspace, &context, input).await
            }
            "fs_stat" => {
                let input: StatInput = parse(arguments)?;
                value(
                    workspace
                        .stat_v2(
                            Some(&context),
                            &chatcmd_runtime::FsStatRequest {
                                path: input.path,
                                version_strength: input.version_strength,
                                hash_algorithm: input.hash_algorithm,
                                budget: input.budget,
                            },
                        )
                        .await?,
                )
            }
            "fs_batch_stat" => {
                let input: chatcmd_runtime::FsBatchStatRequest = parse(arguments)?;
                value(workspace.batch_stat(&context, &input).await?)
            }
            "workspace_index_status" => {
                let input: PathInput = parse(arguments)?;
                value(workspace.index_status(&input.path)?)
            }
            "workspace_index_rebuild" => {
                let input: PathInput = parse(arguments)?;
                value(workspace.rebuild_index(&context, &input.path).await?)
            }
            "fs_create_directory" => {
                let input: PathInput = parse(arguments)?;
                let existed = input.path.exists();
                let entry = workspace.create_directory(&input.path).await?;
                if !existed {
                    self.record_committed_change(
                        &context,
                        &input.path,
                        None,
                        FileChangeKind::DirectoryCreated,
                        Default::default(),
                        capture_snapshot(&input.path),
                        None,
                        None,
                    );
                }
                value(entry)
            }
            "fs_copy" => {
                let input: TransferInput = parse(arguments)?;
                let conflict_policy = transfer_conflict_policy(&input)?;
                let destination = input.destination.clone();
                let destination_existed = destination.exists();
                let before = capture_snapshot(&destination);
                let result = workspace
                    .copy_safe(
                        &context,
                        &FsTransferRequest {
                            source: input.source,
                            destination: input.destination,
                            conflict_policy,
                            atomic_publish: input.atomic_publish,
                            verify: input.verify,
                            preserve_metadata: input.preserve_metadata,
                            follow_symlinks: input.follow_symlinks,
                            dry_run: input.dry_run,
                            expected_source_version: input.expected_source_version,
                            expected_destination_version: input.expected_destination_version,
                            budget: input.budget,
                        },
                    )
                    .await?;
                if result.destination_published && !result.dry_run {
                    self.record_committed_change(
                        &context,
                        &destination,
                        None,
                        if destination_existed {
                            FileChangeKind::Modified
                        } else {
                            FileChangeKind::Added
                        },
                        before,
                        capture_snapshot(&destination),
                        None,
                        result.detail_artifact_ref.clone(),
                    );
                }
                value(result)
            }
            "fs_move" => {
                let input: TransferInput = parse(arguments)?;
                let conflict_policy = transfer_conflict_policy(&input)?;
                let source = input.source.clone();
                let destination = input.destination.clone();
                let before = capture_snapshot(&source);
                let result = workspace
                    .move_safe(
                        &context,
                        &FsTransferRequest {
                            source: input.source,
                            destination: input.destination,
                            conflict_policy,
                            atomic_publish: input.atomic_publish,
                            verify: input.verify,
                            preserve_metadata: input.preserve_metadata,
                            follow_symlinks: input.follow_symlinks,
                            dry_run: input.dry_run,
                            expected_source_version: input.expected_source_version,
                            expected_destination_version: input.expected_destination_version,
                            budget: input.budget,
                        },
                    )
                    .await?;
                if result.destination_published && result.source_removed && !result.dry_run {
                    self.record_committed_change(
                        &context,
                        &destination,
                        Some(source),
                        FileChangeKind::Moved,
                        before,
                        capture_snapshot(&destination),
                        None,
                        result.detail_artifact_ref.clone(),
                    );
                }
                value(result)
            }
            "fs_delete" => {
                filesystem_dispatch::delete(self, workspace, &context, parse(arguments)?).await
            }
            "fs_restore_quarantine" => {
                filesystem_dispatch::restore_quarantine(
                    self,
                    workspace,
                    &context,
                    parse(arguments)?,
                )
                .await
            }
            "fs_quarantine_gc" => {
                filesystem_dispatch::quarantine_gc(workspace, &context, parse(arguments)?).await
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
                let output = git
                    .commit_with_options(
                        &cwd,
                        &input.message,
                        input.all,
                        &input.paths,
                        &input.options,
                        context.cancellation.clone(),
                    )
                    .await?;
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
                self.ask_plan_question(&context, input.question, input.options)
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
            "agent_turn_complete" => self.complete_agent_turn(&context, arguments).await,
            _ => Err(RuntimeError::new("tool_not_found", "unknown MCP tool")),
        }
    }

    async fn register_git_artifact(
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
            relative_path: format!("{}{}", super::MANAGED_ARTIFACT_PREFIX, managed.content_ref),
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

    async fn create_artifact(
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
                    let relative_path = row.get::<String, _>("relative_path");
                    let managed = relative_path.starts_with(super::MANAGED_ARTIFACT_PREFIX);
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
        if let Some(content_ref) = artifact
            .relative_path
            .strip_prefix(super::MANAGED_ARTIFACT_PREFIX)
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

fn value_with_usage<T: serde::Serialize>(value: T, usage: ToolUsage) -> RuntimeResult<Value> {
    let mut output = serde_json::to_value(value)
        .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))?;
    let usage = serde_json::to_value(usage)
        .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))?;
    if let Value::Object(object) = &mut output {
        object.insert("usage".to_owned(), usage);
        Ok(output)
    } else {
        Ok(json!({ "result": output, "usage": usage }))
    }
}

fn transfer_conflict_policy(input: &TransferInput) -> RuntimeResult<FsConflictPolicy> {
    match (input.conflict_policy, input.overwrite) {
        (Some(policy), Some(true)) if policy != FsConflictPolicy::Replace => {
            Err(RuntimeError::new(
                "invalid_arguments",
                "overwrite=true conflicts with conflictPolicy",
            ))
        }
        (Some(policy), _) => Ok(policy),
        (None, Some(true)) => Ok(FsConflictPolicy::Replace),
        (None, _) => Ok(FsConflictPolicy::Error),
    }
}

fn context_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}

fn project_folder_required_for_shell() -> RuntimeError {
    RuntimeError::new(
        "project_folder_required",
        "shell working directory requires the task project folder or an explicit absolute working path",
    )
}
