use super::{WorkspaceService, io_error, walk::configured_walker};
use crate::{
    BatchItemError, FsBatchReadItem, FsBatchReadRequest, FsBatchReadResult, FsBatchStatItem,
    FsBatchStatRequest, FsBatchStatResult, FsBatchUsage, FsStatRequest, IndexFreshness,
    OperationContext, RuntimeError, RuntimeResult, TraversalOptions, WorkspaceIndexStatus,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
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
}

#[derive(Default)]
struct RootIndex {
    generation: u64,
    entries: HashMap<PathBuf, IndexedMetadata>,
    indexed_bytes: u64,
    freshness: Option<IndexFreshness>,
    last_error: Option<String>,
}

struct IndexedMetadata {
    _size: u64,
    _modified_at_ns: u128,
    _entry_type: &'static str,
}

impl WorkspaceService {
    pub fn mark_index_stale(&self, path: &Path) {
        let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Ok(mut state) = self.repository_index.state.write() {
            for (root, index) in state.iter_mut() {
                if normalized.starts_with(root) || path.starts_with(root) {
                    index.freshness = Some(IndexFreshness::Stale);
                }
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
        let metadata_limit = request.budget.max_metadata_calls.min(HARD_BATCH_STAT_ITEMS);
        for (index, path) in request.paths.iter().enumerate() {
            if index >= metadata_limit
                || started.elapsed() >= Duration::from_millis(request.budget.timeout_ms.min(60_000))
            {
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
            let remaining_ms = request
                .budget
                .timeout_ms
                .saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
                .max(1);
            let stat_request = FsStatRequest {
                path: path.clone(),
                version_strength: request.version_strength,
                hash_algorithm: None,
                budget: crate::FsStatBudget {
                    timeout_ms: remaining_ms,
                    max_bytes_read: request.budget.max_bytes_read,
                },
            };
            match self.stat_v2(Some(context), &stat_request).await {
                Ok(stat) => items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: true,
                    stat: Some(stat),
                    error: None,
                }),
                Err(error) => items.push(FsBatchStatItem {
                    path: path.clone(),
                    ok: false,
                    stat: None,
                    error: Some(batch_error(error)),
                }),
            }
        }
        let succeeded = items.iter().filter(|item| item.ok).count();
        let generation = self
            .repository_index
            .state
            .read()
            .ok()
            .and_then(|state| state.values().map(|item| item.generation).max());
        Ok(FsBatchStatResult {
            usage: FsBatchUsage {
                requested: items.len(),
                succeeded,
                failed: items.len() - succeeded,
            },
            items,
            // Exact stat outcomes still come from the live filesystem. Publishing the
            // generation is diagnostic only until stale verification is query-integrated.
            index_used: false,
            index_generation: generation,
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
        let per_item_read_budget = request.budget.max_bytes_read
            / u64::try_from(request.requests.len().max(1)).unwrap_or(u64::MAX);
        let batch_timeout = Duration::from_millis(request.budget.timeout_ms.min(60_000));
        for (index, mut item) in request.requests.iter().cloned().enumerate() {
            item.budget.timeout_ms = item.budget.timeout_ms.min(request.budget.timeout_ms);
            item.budget.max_bytes_read = item.budget.max_bytes_read.min(per_item_read_budget);
            let service = self.clone();
            let context = context.clone();
            let semaphore = semaphore.clone();
            set.spawn(async move {
                let path = item.path.clone();
                let result = tokio::time::timeout(batch_timeout, async {
                    let _permit = semaphore.acquire_owned().await.map_err(|_| {
                        RuntimeError::new("batch_cancelled", "batch semaphore closed")
                    })?;
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
    let walker = configured_walker(root, &TraversalOptions::default())?.build();
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
                _size: size,
                _modified_at_ns: modified_at_ns,
                _entry_type: entry_type,
            },
        );
    }
    Ok((entries, indexed_bytes))
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
