use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chatcmd_runtime::{
    ApplyEditsRequest, OperationContext, RuntimeError, RuntimeResult, SearchProgress,
    TextReadBudget, TextReadRange, TextReadRequestV2, WorkspaceService,
};
use serde_json::{Value, json};

use super::{
    RuntimeHost,
    inputs::{DeleteInput, ReplaceTextInput, SearchInput, WriteTextInput},
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
    Ok(arguments)
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
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: WriteTextInput,
) -> RuntimeResult<Value> {
    let before = snapshot(workspace, &input.path).await;
    let entry = workspace
        .write_text(context, &input.path, &input.content, input.overwrite)
        .await?;
    Ok(with_text_diff(
        value(entry)?,
        &input.path,
        before,
        Some(input.content),
    ))
}

pub(super) async fn replace_text(
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: ReplaceTextInput,
) -> RuntimeResult<Value> {
    let before = snapshot(workspace, &input.path).await;
    let entry = workspace
        .replace_text(
            context,
            &input.path,
            &input.old_text,
            &input.new_text,
            input.expected_occurrences,
        )
        .await?;
    let after = snapshot(workspace, &input.path).await;
    Ok(with_text_diff(value(entry)?, &input.path, before, after))
}

pub(super) async fn apply_edits(
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: ApplyEditsRequest,
) -> RuntimeResult<Value> {
    value(workspace.apply_edits(context, &input).await?)
}

pub(super) async fn delete(
    workspace: &WorkspaceService,
    context: &OperationContext,
    input: DeleteInput,
) -> RuntimeResult<Value> {
    let before = snapshot(workspace, &input.path).await;
    let deleted = workspace
        .delete(context, &input.path, input.recursive)
        .await?;
    Ok(with_text_diff(
        json!({ "deleted": deleted }),
        &input.path,
        before,
        Some(String::new()),
    ))
}

fn snapshot_request(path: &std::path::Path) -> TextReadRequestV2 {
    TextReadRequestV2 {
        path: path.to_path_buf(),
        range: TextReadRange::Byte {
            start: 0,
            limit: 1_000_000,
        },
        max_bytes: 1_000_000,
        include_line_endings: true,
        expected_version: None,
        budget: TextReadBudget {
            timeout_ms: 10_000,
            max_bytes_read: 1_000_003,
        },
    }
}

async fn snapshot(workspace: &WorkspaceService, path: &std::path::Path) -> Option<String> {
    let request = snapshot_request(path);
    workspace
        .read_text_v2(None, &request)
        .await
        .ok()
        .map(|value| value.content)
}

#[cfg(test)]
mod tests {
    use super::{resolve_relative_paths, snapshot_request};
    use chatcmd_runtime::TextReadRange;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn snapshot_request_is_bounded_to_one_megabyte() {
        let request = snapshot_request(Path::new("large.txt"));
        assert_eq!(request.max_bytes, 1_000_000);
        assert_eq!(request.budget.max_bytes_read, 1_000_003);
        assert!(matches!(
            request.range,
            TextReadRange::Byte {
                start: 0,
                limit: 1_000_000
            }
        ));
    }

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

fn with_text_diff(
    mut output: Value,
    path: &std::path::Path,
    before: Option<String>,
    after: Option<String>,
) -> Value {
    if before.is_none() && after.is_none() {
        return output;
    }
    if let Value::Object(ref mut object) = output {
        object.insert(
            "__chatcmdDiff".to_owned(),
            json!({
                "path": path,
                "before": before.unwrap_or_default(),
                "after": after.unwrap_or_default()
            }),
        );
    }
    output
}
