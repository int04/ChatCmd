use super::{WorkspaceService, io_error, walk::configured_walker};
use crate::{
    BatchItemError, FsBatchReadItem, FsBatchReadRequest, FsBatchReadResult, FsBatchStatItem,
    FsBatchStatRequest, FsBatchStatResult, FsBatchUsage, FsStatRequest, IndexFreshness,
    OperationContext, RepositoryIndexEntrySnapshot, RepositoryIndexSnapshot, RuntimeError,
    RuntimeResult, TraversalOptions, WorkspaceIndexStatus,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

const INDEX_SCHEMA_VERSION: u32 = 1;
const HARD_BATCH_STAT_ITEMS: usize = 500;
const HARD_BATCH_READ_ITEMS: usize = 50;
const HARD_BATCH_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct RepositoryIndex {
    state: RwLock<HashMap<PathBuf, RootIndex>>,
    rebuild_gates: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct RootIndex {
    generation: u64,
    entries: HashMap<PathBuf, IndexedMetadata>,
    indexed_bytes: u64,
    freshness: Option<IndexFreshness>,
    last_error: Option<String>,
}

#[derive(Clone)]
struct IndexedMetadata {
    size: u64,
    modified_at_ns: u128,
    entry_type: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct IndexedPathCandidate {
    pub path: PathBuf,
    pub size: u64,
    pub modified_at_ns: u128,
    pub entry_type: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct IndexedCandidateSet {
    pub generation: u64,
    pub freshness: IndexFreshness,
    pub entries: Vec<IndexedPathCandidate>,
}

impl WorkspaceService {
    pub fn mark_index_stale(&self, path: &Path) {
        let normalized = canonicalize_with_missing_suffix(path);
        if let Ok(mut state) = self.repository_index.state.write() {
            for (root, index) in state.iter_mut() {
                let target = if normalized.starts_with(root) {
                    normalized.as_path()
                } else if path.starts_with(root) {
                    path
                } else {
                    continue;
                };
                index.generation = index.generation.saturating_add(1);
                match fs::symlink_metadata(target) {
                    Ok(metadata) => {
                        let size = metadata.len();
                        let modified_at_ns = metadata
                            .modified()
                            .ok()
                            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |value| value.as_nanos());
                        let entry_type = metadata_kind(&metadata);
                        let old_size = index.entries.get(target).map_or(0, |entry| entry.size);
                        index.indexed_bytes = index
                            .indexed_bytes
                            .saturating_sub(old_size)
                            .saturating_add(size);
                        index.entries.insert(
                            target.to_path_buf(),
                            IndexedMetadata {
                                size,
                                modified_at_ns,
                                entry_type,
                            },
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        // Keep watcher/native mutation handling bounded. A removed directory can
                        // represent an arbitrarily large subtree, so tombstone only the exact
                        // entry here and let stale-state direct fallback plus reconcile remove
                        // descendants in the next full snapshot.
                        if let Some(removed) = index.entries.remove(target) {
                            index.indexed_bytes = index.indexed_bytes.saturating_sub(removed.size);
                        }
                    }
                    Err(_) => {}
                }
                // Incremental refresh/tombstone is useful immediately, but a watcher event
                // cannot prove that no sibling event was dropped. Reconcile is still required
                // before the generation is trusted as fresh.
                index.freshness = Some(IndexFreshness::Stale);
            }
        }
    }

    pub async fn rebuild_index(
        &self,
        context: &OperationContext,
        path: &Path,
    ) -> RuntimeResult<WorkspaceIndexStatus> {
        let root = self.existing(path)?;
        root.revalidate()?;
        if !root.is_dir() {
            return Err(RuntimeError::new(
                "index_root_not_directory",
                "index root must be a directory",
            ));
        }
        let root_path = root.to_path_buf();
        let rebuild_gate = {
            let mut gates = self
                .repository_index
                .rebuild_gates
                .lock()
                .map_err(lock_error)?;
            gates
                .entry(root_path.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _rebuild_guard = rebuild_gate.lock().await;
        let crawl_root = root_path.clone();
        let cancellation = context.cancellation.clone();
        let built = tokio::task::spawn_blocking(move || build_entries(&crawl_root, &cancellation))
            .await
            .map_err(super::join_error)?;
        match built {
            Ok((entries, indexed_bytes)) => {
                let mut state = self.repository_index.state.write().map_err(lock_error)?;
                let previous = state.get(&root_path).map_or(0, |value| value.generation);
                state.insert(
                    root_path.clone(),
                    RootIndex {
                        generation: previous.saturating_add(1),
                        entries,
                        indexed_bytes,
                        freshness: Some(IndexFreshness::Fresh),
                        last_error: None,
                    },
                );
                drop(state);
                self.index_status(&root_path)
            }
            Err(error) => {
                if let Ok(mut state) = self.repository_index.state.write() {
                    let entry = state.entry(root_path).or_default();
                    entry.freshness = Some(IndexFreshness::Unknown);
                    entry.last_error = Some(error.message.clone());
                }
                Err(error)
            }
        }
    }

    pub fn index_status(&self, path: &Path) -> RuntimeResult<WorkspaceIndexStatus> {
        let root = self.existing(path)?;
        let root_path = root.to_path_buf();
        let state = self.repository_index.state.read().map_err(lock_error)?;
        let value = state.get(&root_path);
        Ok(WorkspaceIndexStatus {
            root: root_path,
            available: value.is_some(),
            generation: value.map_or(0, |item| item.generation),
            freshness: value
                .and_then(|item| item.freshness)
                .unwrap_or(IndexFreshness::Unknown),
            entry_count: value.map_or(0, |item| item.entries.len()),
            indexed_bytes: value.map_or(0, |item| item.indexed_bytes),
            schema_version: INDEX_SCHEMA_VERSION,
            last_error: value.and_then(|item| item.last_error.clone()),
        })
    }

    pub(super) fn fresh_index_candidates_where(
        &self,
        path: &Path,
        include: impl Fn(&Path, &'static str) -> bool,
    ) -> RuntimeResult<Option<IndexedCandidateSet>> {
        let resolved = self.existing(path)?;
        let requested = resolved.to_path_buf();
        let state = self.repository_index.state.read().map_err(lock_error)?;
        let Some((_, index)) = state
            .iter()
            .filter(|(root, _)| requested.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
        else {
            return Ok(None);
        };
        let freshness = index.freshness.unwrap_or(IndexFreshness::Unknown);
        if freshness != IndexFreshness::Fresh {
            return Ok(None);
        }
        let entries = index
            .entries
            .iter()
            .filter(|(candidate, metadata)| {
                candidate.starts_with(&requested) && include(candidate, metadata.entry_type)
            })
            .map(|(candidate, metadata)| IndexedPathCandidate {
                path: candidate.clone(),
                size: metadata.size,
                modified_at_ns: metadata.modified_at_ns,
                entry_type: metadata.entry_type,
            })
            .collect::<Vec<_>>();
        Ok(Some(IndexedCandidateSet {
            generation: index.generation,
            freshness,
            entries,
        }))
    }

    pub(super) fn fresh_index_metadata(
        &self,
        path: &Path,
    ) -> RuntimeResult<Option<(PathBuf, u64, IndexedPathCandidate)>> {
        let resolved = self.existing(path)?;
        let requested = resolved.to_path_buf();
        let state = self.repository_index.state.read().map_err(lock_error)?;
        let Some((root, index)) = state
            .iter()
            .filter(|(root, _)| requested.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
        else {
            return Ok(None);
        };
        if index.freshness != Some(IndexFreshness::Fresh) {
            return Ok(None);
        }
        Ok(index.entries.get(&requested).map(|metadata| {
            (
                root.clone(),
                index.generation,
                IndexedPathCandidate {
                    path: requested,
                    size: metadata.size,
                    modified_at_ns: metadata.modified_at_ns,
                    entry_type: metadata.entry_type,
                },
            )
        }))
    }

    pub(super) fn indexed_candidate_is_current(
        &self,
        candidate: &IndexedPathCandidate,
    ) -> RuntimeResult<bool> {
        let metadata = match fs::symlink_metadata(&candidate.path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_error(error)),
        };
        let modified_at_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        let entry_type = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        Ok(metadata.len() == candidate.size
            && modified_at_ns == candidate.modified_at_ns
            && entry_type == candidate.entry_type)
    }

    pub fn export_index_snapshot(
        &self,
        path: &Path,
    ) -> RuntimeResult<Option<RepositoryIndexSnapshot>> {
        let root = self.existing(path)?;
        let root_path = root.to_path_buf();
        let state = self.repository_index.state.read().map_err(lock_error)?;
        let Some(index) = state.get(&root_path) else {
            return Ok(None);
        };
        let mut entries = Vec::with_capacity(index.entries.len());
        for (absolute_path, metadata) in &index.entries {
            let relative = absolute_path.strip_prefix(&root_path).map_err(|_| {
                RuntimeError::new(
                    "index_snapshot_invalid",
                    "indexed path escaped its workspace root",
                )
            })?;
            entries.push(RepositoryIndexEntrySnapshot {
                relative_path_bytes: path_bytes(relative),
                display_path: relative.to_string_lossy().into_owned(),
                entry_type: metadata.entry_type.to_owned(),
                size_bytes: metadata.size,
                modified_at_ns: metadata.modified_at_ns,
            });
        }
        entries.sort_by(|left, right| left.relative_path_bytes.cmp(&right.relative_path_bytes));
        Ok(Some(RepositoryIndexSnapshot {
            root: root_path,
            generation: index.generation,
            freshness: index.freshness.unwrap_or(IndexFreshness::Unknown),
            indexed_bytes: index.indexed_bytes,
            schema_version: INDEX_SCHEMA_VERSION,
            entries,
        }))
    }

    pub fn restore_index_snapshot(&self, snapshot: RepositoryIndexSnapshot) -> RuntimeResult<()> {
        if snapshot.schema_version != INDEX_SCHEMA_VERSION {
            return Err(RuntimeError::new(
                "index_schema_mismatch",
                format!(
                    "repository index schema {} is not supported by runtime schema {INDEX_SCHEMA_VERSION}",
                    snapshot.schema_version
                ),
            ));
        }
        if snapshot.entries.len() > 1_000_000 {
            return Err(RuntimeError::new(
                "index_entry_limit",
                "repository index snapshot exceeded 1,000,000 entries",
            ));
        }
        let root = self.existing(&snapshot.root)?;
        let root_path = root.to_path_buf();
        let mut entries = HashMap::with_capacity(snapshot.entries.len());
        for entry in snapshot.entries {
            let relative = path_from_bytes(&entry.relative_path_bytes, &entry.display_path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(RuntimeError::new(
                    "index_snapshot_invalid",
                    "repository index snapshot contains an unsafe relative path",
                ));
            }
            let absolute = root_path.join(relative);
            let entry_type = match entry.entry_type.as_str() {
                "file" => "file",
                "directory" => "directory",
                "symlink" => "symlink",
                _ => "other",
            };
            entries.insert(
                absolute,
                IndexedMetadata {
                    size: entry.size_bytes,
                    modified_at_ns: entry.modified_at_ns,
                    entry_type,
                },
            );
        }
        let mut state = self.repository_index.state.write().map_err(lock_error)?;
        state.insert(
            root_path,
            RootIndex {
                generation: snapshot.generation,
                entries,
                indexed_bytes: snapshot.indexed_bytes,
                // A persisted snapshot is only a candidate accelerator after restart. Mark it
                // stale until a bounded reconcile/rebuild verifies the live filesystem.
                freshness: Some(IndexFreshness::Stale),
                last_error: None,
            },
        );
        Ok(())
    }

    pub async fn batch_stat(
        &self,
        context: &OperationContext,
        request: &FsBatchStatRequest,
    ) -> RuntimeResult<FsBatchStatResult> {
        let limit = request.max_items.clamp(1, HARD_BATCH_STAT_ITEMS);
        if request.paths.len() > limit {
            return Err(RuntimeError::new(
                "batch_item_limit",
                format!(
                    "batch contains {} paths; limit is {limit}",
                    request.paths.len()
                ),
            ));
        }
        let mut items = Vec::with_capacity(request.paths.len());
        let started = Instant::now();
        let timeout = Duration::from_millis(request.budget.timeout_ms.min(60_000));
        let mut remaining_bytes = request.budget.max_bytes_read;
        let mut index_participated = false;
        let mut index_root = None::<PathBuf>;
        let mut index_generation = None::<u64>;
        let mut mixed_index_roots = false;
        let mut stale_entries_detected = 0_u64;
        let metadata_limit = request.budget.max_metadata_calls.min(HARD_BATCH_STAT_ITEMS);
        for (index, path) in request.paths.iter().enumerate() {
            if context.cancellation.is_cancelled() {
                items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: false,
                    stat: None,
                    error: Some(BatchItemError {
                        code: "batch_cancelled".into(),
                        message: "batch stat was cancelled".into(),
                    }),
                });
                continue;
            }
            if index >= metadata_limit || started.elapsed() >= timeout {
                items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: false,
                    stat: None,
                    error: Some(BatchItemError {
                        code: "batch_budget_exhausted".into(),
                        message: "aggregate metadata or time budget exhausted".into(),
                    }),
                });
                continue;
            }
            if path.exists()
                && let Some((root, generation, candidate)) = self.fresh_index_metadata(path)?
            {
                index_participated = true;
                match index_root.as_ref() {
                    Some(existing) if existing != &root => mixed_index_roots = true,
                    None => {
                        index_root = Some(root);
                        index_generation = Some(generation);
                    }
                    Some(_) if index_generation != Some(generation) => mixed_index_roots = true,
                    Some(_) => {}
                }
                if !self.indexed_candidate_is_current(&candidate)? {
                    stale_entries_detected = stale_entries_detected.saturating_add(1);
                    self.mark_index_stale(path);
                }
            }
            if request.version_strength != crate::VersionStrength::Metadata && remaining_bytes == 0
            {
                items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: false,
                    stat: None,
                    error: Some(BatchItemError {
                        code: "batch_budget_exhausted".into(),
                        message: "aggregate maxBytesRead budget exhausted".into(),
                    }),
                });
                continue;
            }
            let remaining_ms =
                request.budget.timeout_ms.min(60_000).saturating_sub(
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
            if remaining_ms == 0 {
                items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: false,
                    stat: None,
                    error: Some(BatchItemError {
                        code: "batch_budget_exhausted".into(),
                        message: "aggregate time budget exhausted".into(),
                    }),
                });
                continue;
            }
            let stat_request = FsStatRequest {
                path: path.clone(),
                version_strength: request.version_strength,
                hash_algorithm: None,
                budget: crate::FsStatBudget {
                    timeout_ms: remaining_ms,
                    max_bytes_read: remaining_bytes,
                },
            };
            match self.stat_v2(Some(context), &stat_request).await {
                Ok(stat) => {
                    remaining_bytes = remaining_bytes.saturating_sub(stat_hash_read_cost(
                        request.version_strength,
                        stat.size_bytes,
                    ));
                    items.push(FsBatchStatItem {
                        path: path.clone(),
                        ok: true,
                        stat: Some(stat),
                        error: None,
                    });
                }
                Err(error) => {
                    if request.version_strength != crate::VersionStrength::Metadata {
                        // A failed/cancelled hash may have consumed any prefix up to the
                        // remaining per-call allowance. Conservatively retire the remainder
                        // so the aggregate hard cap cannot be exceeded by later items.
                        remaining_bytes = 0;
                    }
                    items.push(FsBatchStatItem {
                        path: path.clone(),
                        ok: false,
                        stat: None,
                        error: Some(batch_error(error)),
                    });
                }
            }
        }
        let succeeded = items.iter().filter(|item| item.ok).count();
        let index_used = index_participated && !mixed_index_roots;
        Ok(FsBatchStatResult {
            usage: FsBatchUsage {
                requested: items.len(),
                succeeded,
                failed: items.len() - succeeded,
            },
            items,
            index_used,
            index_generation: index_used.then_some(index_generation).flatten(),
            index_freshness: if stale_entries_detected > 0 {
                IndexFreshness::Stale
            } else if index_used {
                IndexFreshness::Fresh
            } else {
                IndexFreshness::Unknown
            },
            stale_entries_detected,
        })
    }

    pub async fn batch_read(
        &self,
        context: &OperationContext,
        request: &FsBatchReadRequest,
    ) -> RuntimeResult<FsBatchReadResult> {
        let limit = request.max_items.clamp(1, HARD_BATCH_READ_ITEMS);
        if request.requests.len() > limit {
            return Err(RuntimeError::new(
                "batch_item_limit",
                format!(
                    "batch contains {} requests; limit is {limit}",
                    request.requests.len()
                ),
            ));
        }
        let output_limit = request
            .max_total_output_bytes
            .clamp(1, HARD_BATCH_OUTPUT_BYTES);
        let semaphore = std::sync::Arc::new(Semaphore::new(request.concurrency.clamp(1, 16)));
        let mut set = tokio::task::JoinSet::new();
        let batch_timeout = Duration::from_millis(request.budget.timeout_ms.min(60_000));
        let deadline = Instant::now() + batch_timeout;
        let mut unassigned_read_budget = request.budget.max_bytes_read;
        let total_requests = request.requests.len();
        for (index, mut item) in request.requests.iter().cloned().enumerate() {
            let remaining_items = total_requests.saturating_sub(index).max(1);
            let divisor = u64::try_from(remaining_items).unwrap_or(u64::MAX).max(1);
            let fair_share =
                unassigned_read_budget.saturating_add(divisor.saturating_sub(1)) / divisor;
            let assigned = item.budget.max_bytes_read.min(fair_share);
            unassigned_read_budget = unassigned_read_budget.saturating_sub(assigned);
            item.budget.max_bytes_read = assigned;
            item.budget.timeout_ms = item
                .budget
                .timeout_ms
                .min(request.budget.timeout_ms.min(60_000));
            let service = self.clone();
            let context = context.clone();
            let semaphore = semaphore.clone();
            set.spawn(async move {
                let path = item.path.clone();
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return (
                        index,
                        path,
                        Err(RuntimeError::new(
                            "batch_timeout",
                            "aggregate batch read timeout elapsed",
                        )),
                    );
                }
                let cancellation = context.cancellation.clone();
                let result = tokio::time::timeout(remaining, async {
                    if cancellation.is_cancelled() {
                        return Err(RuntimeError::new(
                            "batch_cancelled",
                            "batch read was cancelled",
                        ));
                    }
                    let _permit = tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Err(RuntimeError::new(
                                "batch_cancelled",
                                "batch read was cancelled",
                            ));
                        }
                        permit = semaphore.acquire_owned() => permit.map_err(|_| {
                            RuntimeError::new("batch_cancelled", "batch semaphore closed")
                        })?,
                    };
                    service.read_text_v2(Some(&context), &item).await
                })
                .await
                .unwrap_or_else(|_| {
                    Err(RuntimeError::new(
                        "batch_timeout",
                        "aggregate batch read timeout elapsed",
                    ))
                });
                (index, path, result)
            });
        }
        let mut ordered = vec![None; request.requests.len()];
        while let Some(joined) = set.join_next().await {
            let (index, path, result) = joined.map_err(super::join_error)?;
            ordered[index] = Some((path, result));
        }
        let mut output_bytes = 0usize;
        let mut truncated = false;
        let mut items = Vec::with_capacity(ordered.len());
        for entry in ordered.into_iter().flatten() {
            let (path, result) = entry;
            match result {
                Ok(result) if output_bytes.saturating_add(result.content.len()) <= output_limit => {
                    output_bytes += result.content.len();
                    items.push(FsBatchReadItem {
                        path,
                        ok: true,
                        result: Some(result),
                        error: None,
                    });
                }
                Ok(_) => {
                    truncated = true;
                    items.push(FsBatchReadItem {
                        path,
                        ok: false,
                        result: None,
                        error: Some(BatchItemError {
                            code: "batch_output_limit".into(),
                            message: "aggregate output byte limit reached".into(),
                        }),
                    });
                }
                Err(error) => items.push(FsBatchReadItem {
                    path,
                    ok: false,
                    result: None,
                    error: Some(batch_error(error)),
                }),
            }
        }
        let succeeded = items.iter().filter(|item| item.ok).count();
        Ok(FsBatchReadResult {
            usage: FsBatchUsage {
                requested: items.len(),
                succeeded,
                failed: items.len() - succeeded,
            },
            items,
            output_bytes,
            truncated,
        })
    }
}

fn build_entries(
    root: &Path,
    cancellation: &tokio_util::sync::CancellationToken,
) -> RuntimeResult<(HashMap<PathBuf, IndexedMetadata>, u64)> {
    let mut entries = HashMap::new();
    let mut indexed_bytes = 0u64;
    let walker = configured_walker(
        root,
        &TraversalOptions {
            include_hidden: true,
            ..TraversalOptions::default()
        },
    )?
    .build();
    for entry in walker {
        if cancellation.is_cancelled() {
            return Err(RuntimeError::new(
                "operationCancelled",
                "repository index rebuild was cancelled",
            ));
        }
        if entries.len() >= 1_000_000 {
            return Err(RuntimeError::new(
                "index_entry_limit",
                "repository index rebuild exceeded 1,000,000 entries",
            ));
        }
        let entry =
            entry.map_err(|error| RuntimeError::new("index_walk_failed", error.to_string()))?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        let size = metadata.len();
        indexed_bytes = indexed_bytes.saturating_add(size);
        let modified_at_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        let entry_type = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        entries.insert(
            entry.path().to_path_buf(),
            IndexedMetadata {
                size,
                modified_at_ns,
                entry_type,
            },
        );
    }
    Ok((entries, indexed_bytes))
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8], _display_path: &str) -> PathBuf {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt as _};
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn path_from_bytes(_bytes: &[u8], display_path: &str) -> PathBuf {
    PathBuf::from(display_path)
}

fn canonicalize_with_missing_suffix(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut cursor = path;
    let mut suffix = Vec::new();
    loop {
        if let Some(name) = cursor.file_name() {
            suffix.push(name.to_os_string());
        }
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        if let Ok(mut canonical_parent) = parent.canonicalize() {
            for component in suffix.iter().rev() {
                canonical_parent.push(component);
            }
            return canonical_parent;
        }
        cursor = parent;
    }
}

fn metadata_kind(metadata: &fs::Metadata) -> &'static str {
    if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
}

fn stat_hash_read_cost(strength: crate::VersionStrength, size_bytes: u64) -> u64 {
    match strength {
        crate::VersionStrength::Metadata => 0,
        crate::VersionStrength::Sampled => size_bytes.min(64 * 1024).saturating_mul(3),
        crate::VersionStrength::Content => size_bytes,
    }
}

fn batch_error(error: RuntimeError) -> BatchItemError {
    BatchItemError {
        code: error.code,
        message: error.message,
    }
}
fn lock_error<T>(_: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::new("index_lock_poisoned", "repository index lock is poisoned")
}
