use super::*;

impl RuntimeHost {
    pub(super) async fn dispatch_filesystem_tool(
        &self,
        tool: &str,
        context: &OperationContext,
        arguments: Value,
        workspace: &chatcmd_runtime::WorkspaceService,
    ) -> RuntimeResult<Value> {
        match tool {
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
                        context,
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
                filesystem_dispatch::search(self, workspace, context, parse(arguments)?).await
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
                        context,
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
                value(workspace.read_text_v2(Some(context), &input).await?)
            }
            "fs_batch_read" => {
                let input: chatcmd_runtime::FsBatchReadRequest = parse(arguments)?;
                value(workspace.batch_read(context, &input).await?)
            }
            "fs_write_text" => {
                filesystem_dispatch::write_text(self, workspace, context, parse(arguments)?).await
            }
            "fs_replace_text" => {
                filesystem_dispatch::replace_text(self, workspace, context, parse(arguments)?).await
            }
            "fs_apply_edits" => {
                filesystem_dispatch::apply_edits(self, workspace, context, parse(arguments)?).await
            }
            "fs_write_raw" => {
                let input: WriteRawInput = parse(arguments)?;
                filesystem_dispatch::write_raw(self, workspace, context, input).await
            }
            "fs_stat" => {
                let input: StatInput = parse(arguments)?;
                value(
                    workspace
                        .stat_v2(
                            Some(context),
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
                value(workspace.batch_stat(context, &input).await?)
            }
            "workspace_index_status" => {
                let input: PathInput = parse(arguments)?;
                value(workspace.index_status(&input.path)?)
            }
            "workspace_index_rebuild" => {
                let input: PathInput = parse(arguments)?;
                let status = workspace.rebuild_index(context, &input.path).await?;
                self.persist_repository_index(workspace, &input.path)
                    .await?;
                value(status)
            }
            "fs_create_directory" => {
                let input: PathInput = parse(arguments)?;
                let existed = input.path.exists();
                let entry = workspace.create_directory(&input.path).await?;
                if !existed {
                    self.record_committed_change(
                        context,
                        &input.path,
                        None,
                        FileChangeKind::DirectoryCreated,
                        Default::default(),
                        capture_snapshot(&input.path),
                        None,
                        None,
                    );
                    self.mark_repository_index_stale_for_path(workspace, &input.path)
                        .await?;
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
                        context,
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
                        context,
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
                    self.mark_repository_index_stale_for_path(workspace, &destination)
                        .await?;
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
                        context,
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
                        context,
                        &destination,
                        Some(source.clone()),
                        FileChangeKind::Moved,
                        before,
                        capture_snapshot(&destination),
                        None,
                        result.detail_artifact_ref.clone(),
                    );
                    self.mark_repository_index_stale_for_path(workspace, &source)
                        .await?;
                    self.mark_repository_index_stale_for_path(workspace, &destination)
                        .await?;
                }
                value(result)
            }
            "fs_delete" => {
                filesystem_dispatch::delete(self, workspace, context, parse(arguments)?).await
            }
            "fs_restore_quarantine" => {
                filesystem_dispatch::restore_quarantine(self, workspace, context, parse(arguments)?)
                    .await
            }
            "fs_quarantine_gc" => {
                filesystem_dispatch::quarantine_gc(workspace, context, parse(arguments)?).await
            }
            _ => Err(RuntimeError::new(
                "tool_not_found",
                "unknown filesystem tool",
            )),
        }
    }
}
