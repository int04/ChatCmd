use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::JoinHandle,
    time::{Duration, UNIX_EPOCH},
};

use chatcmd_runtime::{OperationContext, WorkspaceIgnorePolicy};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::Value;

use super::RuntimeHost;

const MAX_TEXT_SNAPSHOT_BYTES: usize = 200_000;
const WATCHER_CHANNEL_CAPACITY: usize = 4_096;
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(100);
const MAX_WATCHER_EVENTS_PER_BATCH: usize = 8_192;

#[derive(Clone, Default)]
pub(super) struct TurnFileChangeTracker {
    state: Arc<Mutex<HashMap<String, TurnState>>>,
}

struct TurnState {
    root: PathBuf,
    watcher: Option<RecommendedWatcher>,
    watcher_worker: Option<JoinHandle<()>>,
    dropped_events: Arc<AtomicU64>,
    changes: BTreeMap<PathBuf, FileChangeRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Moved,
    DirectoryCreated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ChangeOrigin {
    NativeTool,
    ShellWatcher,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ChangeConfidence {
    Exact,
    Sampled,
    MetadataOnly,
    UnknownDueToOverflow,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DiffPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
    binary: bool,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileChangeRecord {
    path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_path: Option<PathBuf>,
    kind: FileChangeKind,
    origin: ChangeOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    additions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<DiffPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_artifact_ref: Option<String>,
    confidence: ChangeConfidence,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Snapshot {
    text: Option<String>,
    size: Option<u64>,
    version: Option<String>,
    binary: bool,
    truncated: bool,
}

struct RawWatcherEvent {
    kind: EventKind,
    paths: Vec<PathBuf>,
}

impl RuntimeHost {
    /// Registers a turn without allocating an OS watcher.
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
                    root,
                    watcher: None,
                    watcher_worker: None,
                    dropped_events: Arc::new(AtomicU64::new(0)),
                    changes: BTreeMap::new(),
                },
            );
        }
    }

    /// Enables the recursive fallback immediately before a shell may mutate files.
    pub(super) fn enable_shell_file_watcher(&self, context: &OperationContext) {
        let Some(turn_id) = context.turn_id.as_deref() else {
            return;
        };
        let (root, dropped_events) = {
            let Ok(state) = self.file_changes.state.lock() else {
                return;
            };
            let Some(turn) = state.get(turn_id) else {
                return;
            };
            if turn.watcher.is_some() {
                return;
            }
            (turn.root.clone(), Arc::clone(&turn.dropped_events))
        };
        let (sender, receiver) = sync_channel(WATCHER_CHANNEL_CAPACITY);
        let weak = Arc::downgrade(&self.file_changes.state);
        let tracked_turn = turn_id.to_owned();
        let worker = std::thread::spawn(move || watcher_worker(weak, tracked_turn, receiver));
        let callback_drops = Arc::clone(&dropped_events);
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                if let Ok(event) = result {
                    enqueue_watcher_event(&sender, &callback_drops, event);
                } else {
                    callback_drops.fetch_add(1, Ordering::Relaxed);
                }
            },
            notify::Config::default(),
        );
        let Ok(mut watcher) = watcher else {
            dropped_events.fetch_add(1, Ordering::Relaxed);
            drop(worker);
            return;
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            dropped_events.fetch_add(1, Ordering::Relaxed);
            drop(watcher);
            let _ = worker.join();
            return;
        }
        if let Ok(mut state) = self.file_changes.state.lock()
            && let Some(turn) = state.get_mut(turn_id)
        {
            turn.watcher = Some(watcher);
            turn.watcher_worker = Some(worker);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_committed_change(
        &self,
        context: &OperationContext,
        path: &Path,
        previous_path: Option<PathBuf>,
        kind: FileChangeKind,
        before: Snapshot,
        after: Snapshot,
        line_delta: Option<(u64, u64)>,
        diff_artifact_ref: Option<String>,
    ) {
        self.workspace.mark_index_stale(path);
        if let Some(previous_path) = previous_path.as_deref() {
            self.workspace.mark_index_stale(previous_path);
        }
        let Some(turn_id) = context.turn_id.as_deref() else {
            return;
        };
        let Ok(mut state) = self.file_changes.state.lock() else {
            return;
        };
        let Some(turn) = state.get_mut(turn_id) else {
            return;
        };
        let path = normalized_path(&turn.root, path);
        if should_ignore_path(&turn.root, &path) {
            return;
        }
        let (additions, deletions) = line_delta
            .map(|(a, d)| (Some(a), Some(d)))
            .unwrap_or_else(|| text_line_delta(&before, &after));
        let record = FileChangeRecord {
            path: path.clone(),
            previous_path,
            kind,
            origin: ChangeOrigin::NativeTool,
            old_version: before.version.clone(),
            new_version: after.version.clone(),
            old_size: before.size,
            new_size: after.size,
            additions,
            deletions,
            preview: preview(&before, &after),
            diff_artifact_ref,
            confidence: snapshot_confidence(&before, &after),
        };
        merge_record(&mut turn.changes, path, record);
    }

    pub(super) async fn finish_turn_file_tracking(
        &self,
        context: &OperationContext,
    ) -> (Vec<Value>, bool, u64) {
        let Some(turn_id) = context.turn_id.as_deref() else {
            return (Vec::new(), false, 0);
        };
        let (watcher, worker) = if let Ok(mut state) = self.file_changes.state.lock() {
            state
                .get_mut(turn_id)
                .map(|turn| (turn.watcher.take(), turn.watcher_worker.take()))
                .unwrap_or_default()
        } else {
            (None, None)
        };
        drop(watcher);
        if let Some(worker) = worker {
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
        }
        let turn = self
            .file_changes
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.remove(turn_id));
        let Some(mut turn) = turn else {
            return (Vec::new(), false, 0);
        };
        let dropped = turn.dropped_events.load(Ordering::Relaxed);
        if dropped > 0 {
            for record in turn.changes.values_mut() {
                if record.origin == ChangeOrigin::ShellWatcher {
                    record.confidence = ChangeConfidence::UnknownDueToOverflow;
                }
            }
        }
        let values = turn
            .changes
            .into_values()
            .filter_map(|record| serde_json::to_value(record).ok())
            .collect();
        (values, dropped > 0, dropped)
    }

    async fn turn_workspace_root(&self, context: &OperationContext) -> Option<PathBuf> {
        if let Ok(Some(folder)) =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await
        {
            let path = PathBuf::from(folder);
            if path.is_dir() {
                return std::fs::canonicalize(&path).ok().or(Some(path));
            }
        }
        None
    }
}

pub(super) fn capture_snapshot(path: &Path) -> Snapshot {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Snapshot::default();
    };
    let size = metadata.len();
    let version = metadata_version(&metadata);
    if !metadata.is_file() {
        return Snapshot {
            size: Some(size),
            version: Some(version),
            ..Snapshot::default()
        };
    }
    let Ok(mut file) = File::open(path) else {
        return Snapshot {
            size: Some(size),
            version: Some(version),
            ..Snapshot::default()
        };
    };
    let mut bytes = Vec::with_capacity(
        MAX_TEXT_SNAPSHOT_BYTES.min(usize::try_from(size).unwrap_or(MAX_TEXT_SNAPSHOT_BYTES)),
    );
    let truncated = size > MAX_TEXT_SNAPSHOT_BYTES as u64;
    if truncated {
        let half = MAX_TEXT_SNAPSHOT_BYTES / 2;
        let mut prefix = vec![0; half];
        let prefix_len = file.read(&mut prefix).unwrap_or(0);
        prefix.truncate(prefix_len);
        let mut suffix = vec![0; half];
        let suffix_len = file
            .seek(SeekFrom::Start(size.saturating_sub(half as u64)))
            .and_then(|_| file.read(&mut suffix))
            .unwrap_or(0);
        suffix.truncate(suffix_len);
        bytes.extend(prefix);
        bytes.extend_from_slice(b"\n... [bounded snapshot] ...\n");
        bytes.extend(suffix);
    } else if file.read_to_end(&mut bytes).is_err() {
        bytes.clear();
    }
    match String::from_utf8(bytes) {
        Ok(text) => Snapshot {
            text: Some(text),
            size: Some(size),
            version: Some(version),
            binary: false,
            truncated,
        },
        Err(_) => Snapshot {
            text: None,
            size: Some(size),
            version: Some(version),
            binary: true,
            truncated,
        },
    }
}

fn metadata_version(metadata: &std::fs::Metadata) -> String {
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    format!("metadata:{}:{modified_ns}", metadata.len())
}

fn enqueue_watcher_event(sender: &SyncSender<RawWatcherEvent>, dropped: &AtomicU64, event: Event) {
    let raw = RawWatcherEvent {
        kind: event.kind,
        paths: event.paths,
    };
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = sender.try_send(raw) {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn watcher_worker(
    state: Weak<Mutex<HashMap<String, TurnState>>>,
    turn_id: String,
    receiver: Receiver<RawWatcherEvent>,
) {
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        while batch.len() < MAX_WATCHER_EVENTS_PER_BATCH {
            match receiver.recv_timeout(WATCHER_DEBOUNCE) {
                Ok(event) => batch.push(event),
                Err(_) => break,
            }
        }
        record_watcher_batch(&state, &turn_id, batch);
    }
}

fn record_watcher_batch(
    state: &Weak<Mutex<HashMap<String, TurnState>>>,
    turn_id: &str,
    batch: Vec<RawWatcherEvent>,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let root = {
        let Ok(state) = state.lock() else {
            return;
        };
        let Some(turn) = state.get(turn_id) else {
            return;
        };
        turn.root.clone()
    };
    let mut coalesced = BTreeMap::<PathBuf, FileChangeKind>::new();
    for event in batch {
        let Some(kind) = watcher_kind(&event.kind) else {
            continue;
        };
        for path in event.paths.into_iter().take(MAX_WATCHER_EVENTS_PER_BATCH) {
            let path = normalized_path(&root, &path);
            if should_ignore_path(&root, &path) {
                continue;
            }
            coalesced
                .entry(path)
                .and_modify(|current| *current = coalesce_kind(*current, kind))
                .or_insert(kind);
        }
    }
    // Snapshot outside the tracker mutex: slow or locked files cannot block
    // native records or another concurrent turn.
    let captured = coalesced
        .into_iter()
        .filter_map(|(path, kind)| {
            let after = capture_snapshot(&path);
            // A path created and removed entirely inside the debounce window is
            // not a user-visible turn change (this also removes atomic staging files).
            if kind == FileChangeKind::Added && after.size.is_none() {
                None
            } else {
                Some((path, kind, after))
            }
        })
        .collect::<Vec<_>>();
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(turn) = state.get_mut(turn_id) else {
        return;
    };
    for (path, kind, after) in captured {
        if turn
            .changes
            .get(&path)
            .is_some_and(|record| record.origin == ChangeOrigin::NativeTool)
        {
            continue;
        }
        if kind == FileChangeKind::Deleted
            && turn
                .changes
                .get(&path)
                .is_some_and(|record| record.kind == FileChangeKind::Added)
        {
            turn.changes.remove(&path);
            continue;
        }
        let record = FileChangeRecord {
            path: path.clone(),
            previous_path: None,
            kind,
            origin: ChangeOrigin::ShellWatcher,
            old_version: None,
            new_version: after.version.clone(),
            old_size: None,
            new_size: after.size,
            additions: None,
            deletions: None,
            preview: preview(&Snapshot::default(), &after),
            diff_artifact_ref: None,
            confidence: if after.text.is_some() {
                ChangeConfidence::Sampled
            } else {
                ChangeConfidence::MetadataOnly
            },
        };
        merge_record(&mut turn.changes, path, record);
    }
}

fn merge_record(
    changes: &mut BTreeMap<PathBuf, FileChangeRecord>,
    path: PathBuf,
    mut next: FileChangeRecord,
) {
    let Some(previous) = changes.remove(&path) else {
        changes.insert(path, next);
        return;
    };
    if previous.kind == FileChangeKind::Added && next.kind == FileChangeKind::Deleted {
        return;
    }
    next.old_version = previous.old_version;
    next.old_size = previous.old_size;
    if let (Some(old_preview), Some(new_preview)) = (previous.preview, next.preview.as_mut()) {
        new_preview.before = old_preview.before;
        new_preview.binary |= old_preview.binary;
        new_preview.truncated |= old_preview.truncated;
    }
    if previous.kind == FileChangeKind::Added {
        next.kind = FileChangeKind::Added;
    }
    changes.insert(path, next);
}

fn watcher_kind(kind: &EventKind) -> Option<FileChangeKind> {
    match kind {
        EventKind::Create(_) => Some(FileChangeKind::Added),
        EventKind::Remove(_) => Some(FileChangeKind::Deleted),
        EventKind::Modify(_) => Some(FileChangeKind::Modified),
        _ => None,
    }
}
fn coalesce_kind(current: FileChangeKind, next: FileChangeKind) -> FileChangeKind {
    match (current, next) {
        (FileChangeKind::Added, FileChangeKind::Modified) => FileChangeKind::Added,
        (_, value) => value,
    }
}
fn normalized_path(root: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}
fn should_ignore_path(root: &Path, path: &Path) -> bool {
    if !path.starts_with(root) {
        return true;
    }
    let policy = WorkspaceIgnorePolicy;
    path.strip_prefix(root)
        .ok()
        .into_iter()
        .flat_map(Path::components)
        .any(|component| policy.should_ignore_default(Path::new(component.as_os_str())))
}
fn snapshot_confidence(before: &Snapshot, after: &Snapshot) -> ChangeConfidence {
    if before.binary || after.binary || (before.text.is_none() && after.text.is_none()) {
        ChangeConfidence::MetadataOnly
    } else if before.truncated || after.truncated {
        ChangeConfidence::Sampled
    } else {
        ChangeConfidence::Exact
    }
}
fn preview(before: &Snapshot, after: &Snapshot) -> Option<DiffPreview> {
    if before.text.is_none() && after.text.is_none() && !before.binary && !after.binary {
        return None;
    }
    Some(DiffPreview {
        before: before.text.clone(),
        after: after.text.clone(),
        binary: before.binary || after.binary,
        truncated: before.truncated || after.truncated,
    })
}
fn text_line_delta(before: &Snapshot, after: &Snapshot) -> (Option<u64>, Option<u64>) {
    if before.truncated || after.truncated {
        return (None, None);
    }
    let (Some(before), Some(after)) = (before.text.as_deref(), after.text.as_deref()) else {
        return (None, None);
    };
    let (a, d) = line_delta(before, after);
    (Some(a as u64), Some(d as u64))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn large_snapshot_is_bounded() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.as_file_mut()
            .set_len(1024 * 1024 * 1024)
            .expect("create sparse 1 GiB fixture");
        file.write_all(b"start").expect("write prefix");
        file.seek(SeekFrom::End(-3)).expect("seek suffix");
        file.write_all(b"end").expect("write suffix");
        let snapshot = capture_snapshot(file.path());
        assert_eq!(snapshot.size, Some(1024 * 1024 * 1024));
        assert!(snapshot.truncated);
        assert!(snapshot.text.expect("text").len() < MAX_TEXT_SNAPSHOT_BYTES + 64);
    }
    #[test]
    fn binary_snapshot_is_metadata_only() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&[0xff, 0xfe, 0xfd]).expect("write");
        let snapshot = capture_snapshot(file.path());
        assert!(snapshot.binary);
        assert!(snapshot.text.is_none());
    }
    #[test]
    fn create_then_delete_is_hidden() {
        let path = PathBuf::from("new.txt");
        let mut changes = BTreeMap::new();
        let base = FileChangeRecord {
            path: path.clone(),
            previous_path: None,
            kind: FileChangeKind::Added,
            origin: ChangeOrigin::NativeTool,
            old_version: None,
            new_version: None,
            old_size: None,
            new_size: Some(1),
            additions: Some(1),
            deletions: Some(0),
            preview: None,
            diff_artifact_ref: None,
            confidence: ChangeConfidence::Exact,
        };
        merge_record(&mut changes, path.clone(), base.clone());
        merge_record(
            &mut changes,
            path.clone(),
            FileChangeRecord {
                kind: FileChangeKind::Deleted,
                ..base
            },
        );
        assert!(changes.is_empty());
    }
    #[test]
    fn watcher_queue_is_bounded_and_reports_drops() {
        let (sender, _receiver) = sync_channel(1);
        let dropped = AtomicU64::new(0);
        let event = || Event::new(EventKind::Any);
        enqueue_watcher_event(&sender, &dropped, event());
        enqueue_watcher_event(&sender, &dropped, event());
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn event_storm_memory_is_bounded_by_channel_capacity() {
        let (sender, _receiver) = sync_channel(64);
        let dropped = AtomicU64::new(0);
        for _ in 0..100_000 {
            enqueue_watcher_event(&sender, &dropped, Event::new(EventKind::Any));
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 99_936);
    }

    #[test]
    fn typed_record_uses_versioned_camel_case_schema() {
        let record = FileChangeRecord {
            path: PathBuf::from("changed.txt"),
            previous_path: None,
            kind: FileChangeKind::Modified,
            origin: ChangeOrigin::NativeTool,
            old_version: Some("old".to_owned()),
            new_version: Some("new".to_owned()),
            old_size: Some(1),
            new_size: Some(2),
            additions: Some(1),
            deletions: Some(0),
            preview: None,
            diff_artifact_ref: Some("artifact-1".to_owned()),
            confidence: ChangeConfidence::Exact,
        };
        let value = serde_json::to_value(record).expect("serialize record");
        assert_eq!(value["origin"], "nativeTool");
        assert_eq!(value["confidence"], "exact");
        assert_eq!(value["diffArtifactRef"], "artifact-1");
    }
}
