use super::search::SearchState;
use crate::{OperationContext, RuntimeError, RuntimeResult, ToolWarning};
use ignore::DirEntry;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Instant, UNIX_EPOCH},
};

const MAX_WARNINGS: usize = 20;

pub(super) fn next_file_entry(
    state: &mut SearchState,
    warnings: &mut Vec<ToolWarning>,
) -> RuntimeResult<Option<DirEntry>> {
    loop {
        let entry = if let Some(entry) = state.pending_entry.take() {
            entry
        } else {
            match state.walker.next() {
                Some(Ok(entry)) => entry,
                Some(Err(error)) => {
                    push_warning(warnings, "filesystem_walk_error", error.to_string());
                    continue;
                }
                None => return Ok(None),
            }
        };
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            return Ok(Some(entry));
        }
    }
}

pub(super) fn has_next_file(
    state: &mut SearchState,
    warnings: &mut Vec<ToolWarning>,
) -> RuntimeResult<bool> {
    if state.pending_entry.is_some() {
        return Ok(true);
    }
    if let Some(entry) = next_file_entry(state, warnings)? {
        state.pending_entry = Some(entry);
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn validate_state(
    state: &SearchState,
    root: &Path,
    root_version: &str,
    owner: &str,
    fingerprint: &str,
    expected_root_version: Option<&str>,
) -> RuntimeResult<()> {
    if state.root != root || state.owner != owner || state.request_fingerprint != fingerprint {
        return Err(RuntimeError::new(
            "cursor_scope_mismatch",
            "cursor does not match path, caller, or search options",
        ));
    }
    if expected_root_version != Some(state.root_version.as_str()) {
        return Err(RuntimeError::new(
            "cursor_scope_mismatch",
            "cursor root version does not match its server state",
        ));
    }
    if root_version != state.root_version {
        return Err(RuntimeError::new(
            "cursor_stale",
            "search root changed after cursor issuance; restart search",
        ));
    }
    Ok(())
}

pub(super) fn root_version(path: &Path) -> RuntimeResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(super::io_error)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0_u128, |value| value.as_nanos());
    Ok(format!("{}:{modified}", metadata.len()))
}

pub(super) fn owner_key(context: &OperationContext) -> String {
    format!(
        "{}:{}:{}",
        context.agent_id,
        context.task_id.as_deref().unwrap_or(""),
        context.conversation_scope_id.as_deref().unwrap_or("")
    )
}

pub(super) fn push_warning(warnings: &mut Vec<ToolWarning>, code: &str, message: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(ToolWarning {
            code: code.to_owned(),
            message,
        });
    }
}

pub(super) fn cleanup_expired(states: &mut HashMap<String, SearchState>) {
    let now = Instant::now();
    states.retain(|_, state| state.expires_at > now);
}

pub(super) fn evict_oldest(states: &mut HashMap<String, SearchState>) {
    if let Some(id) = states
        .iter()
        .min_by_key(|(_, state)| state.expires_at)
        .map(|(id, _)| id.clone())
    {
        states.remove(&id);
    }
}
