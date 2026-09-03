use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chatcmd_runtime::{
    ApplyEditsRequest, FsDeleteRequest, MAX_INLINE_BYTES, OperationContext, RuntimeError,
    RuntimeResult, SearchProgress, WorkspaceService,
};
use serde_json::{Value, json};

use super::turn_file_changes::{FileChangeKind, capture_snapshot};
use super::{
    RuntimeHost,
    inputs::{
        ApplyEditsInput, DeleteInput, ReplaceTextInput, SearchInput, WriteRawInput, WriteTextInput,
    },
    value,
};

pub(super) fn resolve_relative_paths(
    mut arguments: Value,
    base: Option<&Path>,
) -> RuntimeResult<Value> {
    let Some(object) = arguments.as_object_mut() else {
        return Ok(arguments);
    };
    for key in ["path", "source", "destination"] {
        let Some(raw) = object.get(key).and_then(Value::as_str) else {
            continue;
        };
        let path = Path::new(raw);
        if path.is_absolute() {
            continue;
        }
        let base = base.ok_or_else(|| {
            RuntimeError::new(
                "project_folder_required",
                "relative filesystem path requires the task project folder or an explicit absolute work path",
            )
        })?;
        object.insert(
            key.to_owned(),
            Value::String(base.join(path).to_string_lossy().into_owned()),
        );
    }
    if let Some(paths) = object.get_mut("paths").and_then(Value::as_array_mut) {
        for value in paths {
            resolve_path_value(value, base)?;
        }
    }
    if let Some(requests) = object.get_mut("requests").and_then(Value::as_array_mut) {
        for request in requests {
            if let Some(value) = request
                .as_object_mut()
                .and_then(|item| item.get_mut("path"))
            {
                resolve_path_value(value, base)?;
            }
        }
    }
    Ok(arguments)
}

fn resolve_path_value(value: &mut Value, base: Option<&Path>) -> RuntimeResult<()> {
    let Some(raw) = value.as_str() else {
        return Ok(());
    };
    let path = Path::new(raw);
    if path.is_absolute() {
        return Ok(());
    }
    let base = base.ok_or_else(|| RuntimeError::new("project_folder_required", "relative filesystem path requires the task project folder or an explicit absolute work path"))?;
    *value = Value::String(base.join(path).to_string_lossy().into_owned());
    Ok(())
}

pub(super) async fn search(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: SearchInput,
) -> RuntimeResult<Value> {
    let started = Instant::now();
    let scope = workspace.stat(&input.path).await?.path;
    let normalized_scope = scope.to_string_lossy();
    let cursor_state = input
        .cursor
        .as_deref()
        .map(|cursor| {
            host.cursor_codec
                .decode::<chatcmd_runtime::FsSearchCursorState>(
                    cursor,
                    "fs_search",
                    normalized_scope.as_ref(),
                )
        })
        .transpose()?;

    let mut budget = input.budget.unwrap_or_default();
    if let Some(max_file_bytes) = input.max_file_bytes {
        budget.max_file_bytes = max_file_bytes;
    }
    let request = chatcmd_runtime::FsSearchRequest {
        path: input.path,
        query: input.query,
        mode: input.mode.unwrap_or(chatcmd_runtime::SearchMode::Literal),
        case_sensitive: input.case_sensitive,
        word_boundary: input.word_boundary,
        include: input.include,
        exclude: input.exclude,
        include_ignored: input.include_ignored,
        context_before: input.context_before.min(100),
        context_after: input.context_after.min(100),
        max_matches_per_file: input.max_matches_per_file.unwrap_or(50).clamp(1, 5_000),
        limit: input
            .limit
            .or(input.max_results)
            .unwrap_or(200)
            .clamp(1, 5_000),
        max_snippet_bytes: input
            .max_snippet_bytes
            .unwrap_or(8 * 1024)
            .clamp(64, 256 * 1024),
        budget,
    };

    let publisher = host.clone();
    let progress_context = context.clone();
    let progress_sequence = Arc::new(AtomicU64::new(0));
    let last_progress = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
    let (page, state_id) = workspace
        .search_v2(
            context,
            &request,
            cursor_state.as_ref().map(|state| state.state_id.as_str()),
            cursor_state
                .as_ref()
                .map(|state| state.root_version.as_str()),
            move |progress: SearchProgress| {
                let mut last = last_progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if last.elapsed() < Duration::from_millis(250) {
                    return;
                }
                *last = Instant::now();
                let sequence = progress_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                publisher.publish_event(
                    format!("{}:search-progress:{sequence}", progress_context.request_id),
                    "terminal_output",
                    progress_context.task_id.clone(),
                    progress_context.mcp_session_id.clone(),
                    progress_context.turn_id.clone(),
                    json!({
                        "text": format!(
                            "Scanning {} files · {} bytes · {} matches\n{}\n",
                            progress.files_scanned,
                            progress.bytes_scanned,
                            progress.matches_found,
                            progress.path.display()
                        ),
                        "stream": "tool",
                        "encoding": "utf-8",
                        "activityId": progress_context.request_id
                    }),
                );
            },
        )
        .await?;

    let next_cursor = match (page.has_more, state_id) {
        (true, Some(state_id)) => Some(host.cursor_codec.encode(
            "fs_search",
            normalized_scope.as_ref(),
            &chatcmd_runtime::FsSearchCursorState {
                state_id,
                root_version: page.root_version.clone(),
            },
            None,
        )?),
        _ => None,
    };
    let returned_items = u64::try_from(page.data.matches.len()).unwrap_or(u64::MAX);
    let mut result =
        chatcmd_runtime::ToolResultEnvelope::paged(page.data, next_cursor, page.has_more);
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
        files_scanned: Some(page.files_scanned),
        bytes_read: Some(page.bytes_scanned),
        ..chatcmd_runtime::ToolUsage::default()
    });
    result.measure_output_bytes()?;
    value(result)
}

pub(super) async fn write_text(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: WriteTextInput,
) -> RuntimeResult<Value> {
    let path = input.path.clone();
    let before = capture_snapshot(&path);
    let kind = if path.exists() {
        FileChangeKind::Modified
    } else {
        FileChangeKind::Added
    };
    let options = chatcmd_runtime::AtomicWriteOptions {
        overwrite: input.overwrite,
        expected_version: input.expected_version,
        metadata_policy: input.metadata_policy,
        durability: input.durability,
        require_atomic: input.require_atomic,
    };
    let result = match (input.content, input.content_ref) {
        (Some(content), None) => {
            if content.len() > MAX_INLINE_BYTES {
                return Err(RuntimeError::new(
                    "inlineContentTooLarge",
                    "inline content exceeds 256 KiB; upload with blob_begin/blob_write_chunk/blob_seal and pass contentRef",
                ));
            }
            workspace
                .write_text_atomic(context, &input.path, &content, options.clone())
                .await?
        }
        (None, Some(content_ref)) => {
            let lease = host
                .blob_store
                .lease(context, &content_ref, "fsWriteText")?;
            let result = workspace
                .write_blob_atomic(context, &input.path, lease.path(), options.clone(), true)
                .await;
            match result {
                Ok(entry) => {
                    lease.finish(true)?;
                    entry
                }
                Err(error) => {
                    lease.finish(false)?;
                    return Err(error);
                }
            }
        }
        _ => return Err(content_choice_error("content")),
    };
    let after = capture_snapshot(&path);
    host.record_committed_change(context, &path, None, kind, before, after, None, None);
    value(result)
}

pub(super) async fn write_raw(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: WriteRawInput,
) -> RuntimeResult<Value> {
    let path = input.path.clone();
    let before = capture_snapshot(&path);
    let kind = if path.exists() {
        FileChangeKind::Modified
    } else {
        FileChangeKind::Added
    };
    let options = chatcmd_runtime::AtomicWriteOptions {
        overwrite: input.overwrite,
        expected_version: input.expected_version,
        metadata_policy: input.metadata_policy,
        durability: input.durability,
        require_atomic: input.require_atomic,
    };
    let result = match (input.base64, input.content_ref) {
        (Some(base64), None) => {
            if base64.len() > MAX_INLINE_BYTES.saturating_mul(4) / 3 + 4 {
                return Err(RuntimeError::new(
                    "inlineContentTooLarge",
                    "inline Base64 exceeds the bounded limit; upload with blob_begin/blob_write_chunk/blob_seal and pass contentRef",
                ));
            }
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64)
                .map_err(|_| {
                    RuntimeError::new("invalid_base64", "raw file content is not valid Base64")
                })?;
            workspace
                .write_raw_bytes_atomic(context, &input.path, bytes, options.clone())
                .await?
        }
        (None, Some(content_ref)) => {
            let lease = host.blob_store.lease(context, &content_ref, "fsWriteRaw")?;
            let result = workspace
                .write_blob_atomic(context, &input.path, lease.path(), options.clone(), false)
                .await;
            match result {
                Ok(entry) => {
                    lease.finish(true)?;
                    entry
                }
                Err(error) => {
                    lease.finish(false)?;
                    return Err(error);
                }
            }
        }
        _ => return Err(content_choice_error("base64")),
    };
    let after = capture_snapshot(&path);
    host.record_committed_change(context, &path, None, kind, before, after, None, None);
    value(result)
}

pub(super) async fn replace_text(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: ReplaceTextInput,
) -> RuntimeResult<Value> {
    let before = capture_snapshot(&input.path);
    let entry = workspace
        .replace_text(
            context,
            &input.path,
            &input.old_text,
            &input.new_text,
            input.expected_occurrences,
        )
        .await?;
    let after = capture_snapshot(&input.path);
    host.record_committed_change(
        context,
        &input.path,
        None,
        FileChangeKind::Modified,
        before,
        after,
        None,
        None,
    );
    value(entry)
}

pub(super) async fn apply_edits(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: ApplyEditsInput,
) -> RuntimeResult<Value> {
    let path = input.path.clone();
    let before = capture_snapshot(&path);
    let (edits, lease) = match (input.edits, input.content_ref) {
        (Some(edits), None) => (edits, None),
        (None, Some(content_ref)) => {
            let lease = host
                .blob_store
                .lease(context, &content_ref, "fsApplyEdits")?;
            let bytes = tokio::fs::read(lease.path())
                .await
                .map_err(|error| RuntimeError::new("blobIoError", error.to_string()))?;
            let edits = serde_json::from_slice::<Vec<chatcmd_runtime::TextEdit>>(&bytes).map_err(
                |error| {
                    RuntimeError::new(
                        "invalidBlobContent",
                        format!("contentRef must contain a JSON edits array: {error}"),
                    )
                },
            )?;
            (edits, Some(lease))
        }
        _ => return Err(content_choice_error("edits")),
    };
    let request = ApplyEditsRequest {
        path: input.path,
        expected_version: input.expected_version,
        coordinate_system: input.coordinate_system,
        column_encoding: input.column_encoding,
        edits,
        dry_run: input.dry_run,
        preserve_line_endings: input.preserve_line_endings,
        preserve_bom: input.preserve_bom,
        budget: input.budget,
    };
    let result = workspace.apply_edits(context, &request).await;
    if let Some(lease) = lease {
        lease.finish(result.is_ok())?;
    }
    let result = result?;
    if result.applied && !result.dry_run {
        let after = capture_snapshot(&path);
        host.record_committed_change(
            context,
            &path,
            None,
            FileChangeKind::Modified,
            before,
            after,
            Some((result.additions, result.deletions)),
            result.diff_artifact_ref.clone(),
        );
    }
    value(result)
}

fn content_choice_error(inline_field: &str) -> RuntimeError {
    RuntimeError::new(
        "invalidContentSource",
        format!("provide exactly one of {inline_field} or contentRef"),
    )
}

pub(super) async fn delete(
    host: &RuntimeHost,
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: DeleteInput,
) -> RuntimeResult<Value> {
    let before = capture_snapshot(&input.path);
    let deleted = workspace
        .delete_safe(
            context,
            &FsDeleteRequest {
                path: input.path.clone(),
                recursive: input.recursive,
                mode: input.mode,
                expected_version: input.expected_version,
                dry_run: input.dry_run,
                budget: input.budget,
            },
        )
        .await?;
    if deleted.source_removed && !deleted.dry_run {
        host.record_committed_change(
            context,
            &input.path,
            None,
            FileChangeKind::Deleted,
            before,
            Default::default(),
            None,
            deleted.detail_artifact_ref.clone(),
        );
    }
    serde_json::to_value(deleted)
        .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::resolve_relative_paths;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn relative_filesystem_paths_are_anchored_to_project_folder() {
        let base = Path::new("D:/DEV/CmdGPT/ChatCmdClient");
        let resolved = resolve_relative_paths(
            json!({"path":"src/websocket.rs","source":"web/a.ts","destination":"web/b.ts"}),
            Some(base),
        )
        .expect("resolve relative paths");
        assert_eq!(
            Path::new(resolved["path"].as_str().expect("path")),
            base.join("src/websocket.rs")
        );
        assert_eq!(
            Path::new(resolved["source"].as_str().expect("source")),
            base.join("web/a.ts")
        );
        assert_eq!(
            Path::new(resolved["destination"].as_str().expect("destination")),
            base.join("web/b.ts")
        );
    }

    #[test]
    fn relative_batch_paths_are_anchored_to_project_folder() {
        let base = Path::new("D:/DEV/CmdGPT/ChatCmdClient");
        let resolved = resolve_relative_paths(
            json!({
                "paths": ["src/a.rs", "src/b.rs"],
                "requests": [{"path": "docs/readme.md"}]
            }),
            Some(base),
        )
        .expect("resolve batch paths");
        assert_eq!(
            Path::new(resolved["paths"][0].as_str().expect("stat path")),
            base.join("src/a.rs")
        );
        assert_eq!(
            Path::new(resolved["requests"][0]["path"].as_str().expect("read path")),
            base.join("docs/readme.md")
        );
    }

    #[test]
    fn absolute_filesystem_paths_are_preserved() {
        let absolute = if cfg!(windows) {
            "D:/DEV/CmdGPT/ChatCmdClient/src/main.rs"
        } else {
            "/tmp/project/src/main.rs"
        };
        let resolved = resolve_relative_paths(json!({"path": absolute}), Some(Path::new(".")))
            .expect("preserve absolute path");
        assert_eq!(resolved["path"], absolute);
    }

    #[test]
    fn relative_path_requires_the_task_project_folder() {
        let error = resolve_relative_paths(json!({"path":"src/main.rs"}), None)
            .expect_err("relative path without base must fail explicitly");
        assert_eq!(error.code, "project_folder_required");
        assert_eq!(
            error.message,
            "relative filesystem path requires the task project folder or an explicit absolute work path"
        );
    }
}
