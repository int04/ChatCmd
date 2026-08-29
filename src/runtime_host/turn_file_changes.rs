use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use chatcmd_runtime::OperationContext;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Value, json};

use super::RuntimeHost;

const MAX_TEXT_SNAPSHOT_BYTES: usize = 200_000;

#[derive(Clone, Default)]
pub(super) struct TurnFileChangeTracker {
    state: Arc<Mutex<HashMap<String, TurnState>>>,
}

struct TurnState {
    root: PathBuf,
    watcher: Option<RecommendedWatcher>,
    changes: BTreeMap<PathBuf, ChangeState>,
}

#[derive(Clone, Default)]
struct ChangeState {
    before: Option<String>,
    after: Option<String>,
    kind_hint: Option<&'static str>,
    exact_before: bool,
    created_in_turn: bool,
    removed_after_create: bool,
}

impl RuntimeHost {
    pub(super) async fn begin_turn_file_tracking(&self, context: &OperationContext) {
        let Some(turn_id) = context.turn_id.as_deref().filter(|value| !value.is_empty()) else {
            return;
        };
        let Some(root) = self.turn_workspace_root(context).await else {
            return;
        };

        if let Ok(mut state) = self.file_changes.state.lock() {
            state.insert(
                turn_id.to_owned(),
                TurnState {
                    root: root.clone(),
                    watcher: None,
                    changes: BTreeMap::new(),
                },
            );
        }

        let weak = Arc::downgrade(&self.file_changes.state);
        let tracked_turn = turn_id.to_owned();
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                if let Ok(event) = result {
                    record_watcher_event(&weak, &tracked_turn, event);
                }
            },
            notify::Config::default(),
        );
        let Ok(mut watcher) = watcher else {
            return;
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            return;
        }
        if let Ok(mut state) = self.file_changes.state.lock()
            && let Some(turn) = state.get_mut(turn_id)
        {
            turn.watcher = Some(watcher);
        }
    }

    pub(super) fn record_tool_diff(&self, context: &OperationContext, output: &Value) {
        let Some(turn_id) = context.turn_id.as_deref() else {
            return;
        };
        let Some(diff) = output.get("__chatcmdDiff").and_then(Value::as_object) else {
            return;
        };
        let Some(path) = diff.get("path").and_then(Value::as_str).map(PathBuf::from) else {
            return;
        };
        let before = diff
            .get("before")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let after = diff
            .get("after")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let Ok(mut state) = self.file_changes.state.lock() else {
            return;
        };
        let Some(turn) = state.get_mut(turn_id) else {
            return;
        };
        let path = absolute_path(&turn.root, &path);
        if should_ignore_path(&turn.root, &path) {
            return;
        }
        let change = turn.changes.entry(path).or_default();
        if !change.exact_before {
            change.before = before;
            change.exact_before = true;
        }
        change.after = after;
    }

    pub(super) async fn finish_turn_file_tracking(&self, context: &OperationContext) -> Vec<Value> {
        let Some(turn_id) = context.turn_id.as_deref() else {
            return Vec::new();
        };
        tokio::time::sleep(std::time::Duration::from_millis(35)).await;
        let turn = self
            .file_changes
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.remove(turn_id));
        let Some(mut turn) = turn else {
            return Vec::new();
        };
        drop(turn.watcher.take());

        let mut result = Vec::new();
        for (path, mut change) in turn.changes {
            if should_ignore_path(&turn.root, &path) {
                continue;
            }
            let current = read_text_snapshot(&path);
            let exists_now = path.is_file();
            if change.created_in_turn && change.removed_after_create && !exists_now {
                continue;
            }
            if change.after.is_none() {
                change.after = current.clone();
            }
            let before = change.before.unwrap_or_default();
            let after = change.after.unwrap_or_default();
            let kind = if !exists_now {
                "deleted"
            } else if change.kind_hint == Some("added") && before.is_empty() {
                "added"
            } else {
                "modified"
            };
            if before == after && change.kind_hint.is_none() {
                continue;
            }
            let (additions, deletions) = if before == after {
                (0, 0)
            } else {
                line_delta(&before, &after)
            };
            result.push(json!({
                "path": path,
                "kind": kind,
                "additions": additions,
                "deletions": deletions,
                "before": before,
                "after": after,
                "beforeAvailable": change.exact_before || kind == "added"
            }));
        }
        result
    }

    async fn turn_workspace_root(&self, context: &OperationContext) -> Option<PathBuf> {
        if let Ok(Some(project_folder)) =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, &context.agent_id).await
        {
            let path = PathBuf::from(project_folder);
            if path.is_dir() {
                return Some(path);
            }
        }
        self.workspace
            .roots()
            .first()
            .filter(|path| path.is_dir())
            .cloned()
    }
}

fn record_watcher_event(
    state: &Weak<Mutex<HashMap<String, TurnState>>>,
    turn_id: &str,
    event: Event,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(turn) = state.get_mut(turn_id) else {
        return;
    };
    let hint = event_kind_hint(&event.kind);
    for path in event.paths {
        let path = absolute_path(&turn.root, &path);
        if should_ignore_path(&turn.root, &path) {
            continue;
        }
        if path.is_dir() {
            continue;
        }
        let change = turn.changes.entry(path.clone()).or_default();
        if hint == Some("added") {
            change.created_in_turn = true;
            change.removed_after_create = false;
        } else if hint == Some("deleted") && change.created_in_turn {
            change.removed_after_create = true;
        }
        if change.before.is_none() {
            if hint == Some("added") {
                change.before = Some(String::new());
                change.exact_before = true;
            } else if let Some(snapshot) = read_text_snapshot(&path) {
                change.before = Some(snapshot);
            }
        }
        change.kind_hint = merge_hint(change.kind_hint, hint);
    }
}

fn event_kind_hint(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Create(_) => Some("added"),
        EventKind::Remove(_) => Some("deleted"),
        EventKind::Modify(_) => Some("modified"),
        _ => None,
    }
}

fn merge_hint(current: Option<&'static str>, next: Option<&'static str>) -> Option<&'static str> {
    match (current, next) {
        (_, Some("deleted")) => Some("deleted"),
        (Some("added"), Some("modified")) => Some("added"),
        (_, Some(value)) => Some(value),
        (value, None) => value,
    }
}

fn absolute_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn should_ignore_path(root: &Path, path: &Path) -> bool {
    if !path.starts_with(root) || transient_file(path) {
        return true;
    }
    path.strip_prefix(root)
        .ok()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| component.as_os_str().to_str())
        .any(ignored_component)
}

fn ignored_component(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        ".git"
            | ".idea"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".vite"
            | ".vs"
            | ".gradle"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".parcel-cache"
            | ".svelte-kit"
            | ".angular"
            | ".expo"
            | ".pnpm-store"
            | ".dart_tool"
            | ".symlinks"
            | ".cxx"
            | ".externalnativebuild"
            | ".nyc_output"
            | "artifacts"
            | "bin"
            | "bower_components"
            | "build"
            | "coverage"
            | "deriveddata"
            | "dist"
            | "htmlcov"
            | "jspm_packages"
            | "node_modules"
            | "obj"
            | "pods"
            | "target"
            | "testresults"
    )
}

fn transient_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    lower.starts_with(".tmp_")
        || lower.starts_with("tmp_agent_")
        || is_named_tempfile_name(name)
}

fn is_named_tempfile_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(".tmp") else {
        return false;
    };
    suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::is_named_tempfile_name;

    #[test]
    fn recognizes_tempfile_named_temp_files() {
        assert!(is_named_tempfile_name(".tmp0GV0Gk"));
        assert!(is_named_tempfile_name(".tmp9wyOqd"));
    }

    #[test]
    fn keeps_normal_dot_tmp_files_trackable() {
        assert!(!is_named_tempfile_name(".tmp"));
        assert!(!is_named_tempfile_name(".tmp_config"));
        assert!(!is_named_tempfile_name(".tmp-long-lived-file"));
    }
}

fn read_text_snapshot(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let value = String::from_utf8(bytes).ok()?;
    if value.len() <= MAX_TEXT_SNAPSHOT_BYTES {
        return Some(value);
    }
    let mut end = MAX_TEXT_SNAPSHOT_BYTES;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    Some(format!("{}\n… [truncated]", &value[..end]))
}

fn line_delta(before: &str, after: &str) -> (usize, usize) {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut prefix = 0;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before_lines.len().saturating_sub(prefix)
        && suffix < after_lines.len().saturating_sub(prefix)
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    (
        after_lines.len().saturating_sub(prefix + suffix),
        before_lines.len().saturating_sub(prefix + suffix),
    )
}
