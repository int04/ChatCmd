use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chatcmd_runtime::{OperationContext, RuntimeResult, SearchProgress, WorkspaceService};
use serde_json::{Value, json};

use super::{
    RuntimeHost,
    inputs::{DeleteInput, ReplaceTextInput, SearchInput, WriteTextInput},
    value,
};

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
