use std::{
    collections::HashSet,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chatcmd_runtime::{
    OperationContext, RuntimeError, RuntimeResult, SearchProgress, WorkspaceService,
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
    let publisher = host.clone();
    let progress_context = context.clone();
    let progress_sequence = Arc::new(AtomicU64::new(0));
    let seen_paths = Arc::new(Mutex::new(HashSet::<String>::new()));
    value(
        workspace
            .search(
                &input.path,
                &input.query,
                input.case_sensitive,
                input.max_results,
                input.max_file_bytes,
                input.include_ignored,
                input.exclude,
                move |progress: SearchProgress| {
                    let sequence = progress_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                    let text = if progress.matched.is_some() {
                        let path = progress.path.display().to_string();
                        let mut seen = seen_paths.lock().unwrap_or_else(|error| error.into_inner());
                        if !seen.insert(path.clone()) {
                            return;
                        }
                        format!("{path}\n")
                    } else {
                        format!(
                            "Scanning {} files · {} matches\n{}\n",
                            progress.files_scanned,
                            progress.matches_found,
                            progress.path.display()
                        )
                    };
                    publisher.publish_event(
                        format!("{}:search-progress:{sequence}", progress_context.request_id),
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

async fn snapshot(workspace: &WorkspaceService, path: &std::path::Path) -> Option<String> {
    workspace
        .read_text(path, 1_000_000)
        .await
        .ok()
        .map(|value| value.content)
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
