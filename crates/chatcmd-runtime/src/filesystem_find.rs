use super::{WorkspaceService, walk::configured_walker};
use crate::{
    BudgetTracker, FindEntryType, FindPatternMode, FsFindItem, FsFindPageData, FsFindRequest,
    FsFindScanPage, OperationContext, RuntimeError, RuntimeResult, ToolBudget, ToolWarning,
    TraversalOptions, TruncationReason,
};
use globset::{GlobBuilder, GlobMatcher};
use ignore::Walk;
use regex::{Regex, RegexBuilder};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};
use uuid::Uuid;

const FIND_STATE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ACTIVE_FIND_STATES: usize = 128;
const MAX_WARNINGS: usize = 20;
const HARD_FIND_TIMEOUT: Duration = Duration::from_secs(60);
const HARD_FIND_ENTRIES: u64 = 1_000_000;
const HARD_FIND_METADATA_CALLS: u64 = 100_000;

pub(super) struct FindStateStore {
    states: Mutex<HashMap<String, FindState>>,
}

impl Default for FindStateStore {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

struct FindState {
    root: PathBuf,
    root_version: String,
    owner: String,
    request_fingerprint: String,
    source: FindSource,
    pending: Option<FindCandidate>,
    index_generation: Option<u64>,
    index_freshness: crate::IndexFreshness,
    stale_entries_detected: u64,
    expires_at: Instant,
}

enum FindSource {
    Direct(Box<Walk>),
    Indexed {
        entries: Vec<super::repository_index::IndexedPathCandidate>,
        position: usize,
    },
}

struct FindCandidate {
    path: PathBuf,
    entry_type: &'static str,
    depth: usize,
    indexed: Option<super::repository_index::IndexedPathCandidate>,
    metadata_call: bool,
}

enum CompiledPattern {
    Literal(String),
    Glob(GlobMatcher),
    Regex(Regex),
}

impl WorkspaceService {
    pub async fn find_v2(
        &self,
        context: &OperationContext,
        request: &FsFindRequest,
        state_id: Option<&str>,
        expected_root_version: Option<&str>,
    ) -> RuntimeResult<(FsFindScanPage, Option<String>)> {
        let _admission = self
            .admission
            .try_admit(&context.agent_id, 1, 8 * 1024 * 1024)?;
        let root = self.existing(&request.path)?;
        root.revalidate()?;
        let mut request = request.clone();
        request.budget.timeout_ms = request.budget.timeout_ms.min(60_000);
        request.budget.max_entries_scanned = request
            .budget
            .max_entries_scanned
            .clamp(1, HARD_FIND_ENTRIES);
        request.budget.max_metadata_calls = request
            .budget
            .max_metadata_calls
            .min(HARD_FIND_METADATA_CALLS);
        let tracker = BudgetTracker::new(
            context.cancellation.clone(),
            ToolBudget::intersect([
                &ToolBudget {
                    deadline: Some(Instant::now() + HARD_FIND_TIMEOUT),
                    max_entries: Some(HARD_FIND_ENTRIES),
                    ..ToolBudget::default()
                },
                &ToolBudget {
                    deadline: Some(
                        Instant::now() + Duration::from_millis(request.budget.timeout_ms),
                    ),
                    max_entries: Some(request.budget.max_entries_scanned.max(1)),
                    ..ToolBudget::default()
                },
            ]),
        );
        let owner = owner_key(context);
        let store = self.find_states.clone();
        let service = self.clone();
        let state_id = state_id.map(str::to_owned);
        let expected_root_version = expected_root_version.map(str::to_owned);

        tokio::task::spawn_blocking(move || {
            let matcher = compile_pattern(&request)?;
            let version = root_version(&root)?;
            let fingerprint = serde_json::to_string(&request)
                .map_err(|error| RuntimeError::new("find_request_invalid", error.to_string()))?;
            let mut states = store.states.lock().map_err(|_| {
                RuntimeError::new("find_state_poisoned", "find state lock is poisoned")
            })?;
            cleanup_expired(&mut states);
            let continuing = state_id.is_some();

            let (id, mut state) = if let Some(id) = state_id {
                let state = states.remove(&id).ok_or_else(|| {
                    RuntimeError::new(
                        "cursor_expired",
                        "find cursor state is no longer available; restart traversal",
                    )
                })?;
                validate_state(
                    &state,
                    &root,
                    &version,
                    &owner,
                    &fingerprint,
                    expected_root_version.as_deref(),
                )?;
                if let Some(expected_generation) = state.index_generation {
                    let status = service.index_status(&root)?;
                    if !status.available
                        || status.freshness != crate::IndexFreshness::Fresh
                        || status.generation != expected_generation
                    {
                        return Err(RuntimeError::new(
                            "cursor_stale",
                            "repository index generation changed after cursor issuance; restart find",
                        ));
                    }
                }
                (id, state)
            } else {
                let indexed = if !request.include_ignored && request.exclude.is_empty() {
                    service.fresh_index_candidates_where(&root, |path, entry_type| {
                        let depth = path
                            .strip_prefix(&root)
                            .map_or(usize::MAX, |relative| relative.components().count());
                        depth <= request.max_depth
                            && (request.include_hidden || !path_has_hidden_component(&root, path))
                            && entry_type_matches(entry_type, &request.entry_types)
                            && extension_matches(path, &request.extensions)
                            && pattern_matches(&root, path, &matcher, request.case_sensitive)
                    })?
                } else {
                    None
                };
                let (source, index_generation, index_freshness) = if let Some(indexed) = indexed {
                    (
                        FindSource::Indexed {
                            entries: indexed.entries,
                            position: 0,
                        },
                        Some(indexed.generation),
                        indexed.freshness,
                    )
                } else {
                    let walker = configured_walker(
                        &root,
                        &TraversalOptions {
                            include_hidden: request.include_hidden,
                            include_ignored: request.include_ignored,
                            exclude: request.exclude.clone(),
                            max_depth: request.max_depth,
                            ..TraversalOptions::default()
                        },
                    )?
                    .build();
                    (
                        FindSource::Direct(Box::new(walker)),
                        None,
                        crate::IndexFreshness::Unknown,
                    )
                };
                (
                    Uuid::new_v4().to_string(),
                    FindState {
                        root: root.to_path_buf(),
                        root_version: version.clone(),
                        owner,
                        request_fingerprint: fingerprint,
                        source,
                        pending: None,
                        index_generation,
                        index_freshness,
                        stale_entries_detected: 0,
                        expires_at: Instant::now() + FIND_STATE_TTL,
                    },
                )
            };
            drop(states);

            let page = match scan_page(&mut state, &request, &matcher, &tracker) {
                Ok(page) => page,
                Err(error) if error.code == "index_stale_detected" => {
                    service.mark_index_stale(&root);
                    if continuing {
                        return Err(RuntimeError::new(
                            "cursor_stale",
                            "repository index changed after cursor issuance; restart find",
                        ));
                    }
                    let walker = configured_walker(
                        &root,
                        &TraversalOptions {
                            include_hidden: request.include_hidden,
                            include_ignored: request.include_ignored,
                            exclude: request.exclude.clone(),
                            max_depth: request.max_depth,
                            ..TraversalOptions::default()
                        },
                    )?
                    .build();
                    state.source = FindSource::Direct(Box::new(walker));
                    state.pending = None;
                    state.index_freshness = crate::IndexFreshness::Stale;
                    scan_page(&mut state, &request, &matcher, &tracker)?
                }
                Err(error) => return Err(error),
            };
            if page.has_more {
                state.expires_at = Instant::now() + FIND_STATE_TTL;
                let mut states = store.states.lock().map_err(|_| {
                    RuntimeError::new("find_state_poisoned", "find state lock is poisoned")
                })?;
                cleanup_expired(&mut states);
                if states.len() >= MAX_ACTIVE_FIND_STATES {
                    evict_oldest(&mut states);
                }
                states.insert(id.clone(), state);
                Ok((page, Some(id)))
            } else {
                Ok((page, None))
            }
        })
        .await
        .map_err(super::join_error)?
    }
}

fn scan_page(
    state: &mut FindState,
    request: &FsFindRequest,
    matcher: &CompiledPattern,
    tracker: &BudgetTracker,
) -> RuntimeResult<FsFindScanPage> {
    let limit = request.limit.clamp(1, 5_000);
    let max_entries = request
        .budget
        .max_entries_scanned
        .clamp(1, HARD_FIND_ENTRIES);
    let max_metadata = request
        .budget
        .max_metadata_calls
        .min(HARD_FIND_METADATA_CALLS);
    let mut items = Vec::with_capacity(limit);
    let mut entries_scanned = 0_u64;
    let mut metadata_calls = 0_u64;
    let mut warnings = Vec::new();
    let mut truncation_reason = None;

    while items.len() < limit {
        if let Err(error) = tracker.checkpoint() {
            truncation_reason = Some(if error.code == "operationCancelled" {
                TruncationReason::Cancelled
            } else {
                TruncationReason::TimeBudget
            });
            break;
        }
        if entries_scanned >= max_entries {
            truncation_reason = Some(TruncationReason::ItemLimit);
            break;
        }

        let Some(entry) = next_entry(state, &mut warnings)? else {
            break;
        };
        tracker.record_entries(1);
        entries_scanned = entries_scanned.saturating_add(1);
        if entry.depth == 0 && entry.path == state.root && state.root.is_dir() {
            continue;
        }
        if entry.depth > request.max_depth {
            continue;
        }
        if let Some(indexed) = entry.indexed.as_ref()
            && !indexed_candidate_matches_live(indexed)?
        {
            state.stale_entries_detected = state.stale_entries_detected.saturating_add(1);
            return Err(RuntimeError::new(
                "index_stale_detected",
                "repository index changed during find; retry with direct traversal",
            ));
        }
        if !request.include_hidden && path_has_hidden_component(&state.root, &entry.path) {
            continue;
        }
        if !entry_type_matches(entry.entry_type, &request.entry_types) {
            continue;
        }
        if !extension_matches(&entry.path, &request.extensions) {
            continue;
        }
        if !pattern_matches(&state.root, &entry.path, matcher, request.case_sensitive) {
            continue;
        }
        if entry.metadata_call {
            metadata_calls = metadata_calls.saturating_add(1);
            if metadata_calls > max_metadata {
                state.pending = Some(entry);
                truncation_reason = Some(TruncationReason::MetadataBudget);
                break;
            }
        }
        items.push(FsFindItem {
            path: entry.path.to_string_lossy().into_owned(),
            entry_type: entry.entry_type.to_owned(),
        });
    }

    let mut has_more = truncation_reason.is_some();
    if !has_more && items.len() == limit {
        if let Some(entry) = next_entry(state, &mut warnings)? {
            state.pending = Some(entry);
            has_more = true;
        }
    }

    Ok(FsFindScanPage {
        data: FsFindPageData {
            items,
            index_used: matches!(state.source, FindSource::Indexed { .. }),
            index_generation: state.index_generation,
            index_freshness: state.index_freshness,
            stale_entries_detected: state.stale_entries_detected,
        },
        has_more,
        entries_scanned,
        metadata_calls,
        truncation_reason,
        warnings,
        root_version: state.root_version.clone(),
    })
}

fn next_entry(
    state: &mut FindState,
    warnings: &mut Vec<ToolWarning>,
) -> RuntimeResult<Option<FindCandidate>> {
    if let Some(entry) = state.pending.take() {
        return Ok(Some(entry));
    }
    loop {
        match &mut state.source {
            FindSource::Direct(walker) => match walker.next() {
                Some(Ok(entry)) => {
                    let Some(kind) = entry.file_type() else {
                        let metadata =
                            fs::symlink_metadata(entry.path()).map_err(super::io_error)?;
                        return Ok(Some(FindCandidate {
                            path: entry.path().to_path_buf(),
                            entry_type: metadata_kind(&metadata),
                            depth: entry.depth(),
                            indexed: None,
                            metadata_call: true,
                        }));
                    };
                    return Ok(Some(FindCandidate {
                        path: entry.path().to_path_buf(),
                        entry_type: file_type_kind(kind),
                        depth: entry.depth(),
                        indexed: None,
                        metadata_call: false,
                    }));
                }
                Some(Err(error)) => {
                    push_warning(warnings, "filesystem_walk_error", error.to_string());
                }
                None => return Ok(None),
            },
            FindSource::Indexed { entries, position } => {
                let Some(entry) = entries.get(*position).cloned() else {
                    return Ok(None);
                };
                *position = position.saturating_add(1);
                let depth = entry
                    .path
                    .strip_prefix(&state.root)
                    .map_or(usize::MAX, |relative| relative.components().count());
                return Ok(Some(FindCandidate {
                    path: entry.path.clone(),
                    entry_type: entry.entry_type,
                    depth,
                    indexed: Some(entry),
                    metadata_call: false,
                }));
            }
        }
    }
}

fn compile_pattern(request: &FsFindRequest) -> RuntimeResult<CompiledPattern> {
    match request.pattern_mode {
        FindPatternMode::Literal => Ok(CompiledPattern::Literal(if request.case_sensitive {
            request.pattern.clone()
        } else {
            request.pattern.to_lowercase()
        })),
        FindPatternMode::Glob => GlobBuilder::new(&request.pattern)
            .case_insensitive(!request.case_sensitive)
            .literal_separator(false)
            .build()
            .map(|glob| CompiledPattern::Glob(glob.compile_matcher()))
            .map_err(|error| RuntimeError::new("invalid_find_pattern", error.to_string())),
        FindPatternMode::Regex => RegexBuilder::new(&request.pattern)
            .case_insensitive(!request.case_sensitive)
            .size_limit(1 << 20)
            .dfa_size_limit(2 << 20)
            .build()
            .map(CompiledPattern::Regex)
            .map_err(|error| RuntimeError::new("invalid_find_pattern", error.to_string())),
    }
}

fn pattern_matches(
    root: &Path,
    path: &Path,
    matcher: &CompiledPattern,
    case_sensitive: bool,
) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    match matcher {
        CompiledPattern::Literal(needle) => {
            let name = path.file_name().map_or_else(
                || std::borrow::Cow::Borrowed(""),
                |value| value.to_string_lossy(),
            );
            if case_sensitive {
                name.contains(needle)
            } else {
                name.to_lowercase().contains(needle)
            }
        }
        CompiledPattern::Glob(glob) => glob.is_match(&relative_text),
        CompiledPattern::Regex(regex) => regex.is_match(&relative_text),
    }
}

fn entry_type_matches(entry_type: &str, wanted: &[FindEntryType]) -> bool {
    wanted.is_empty()
        || wanted.iter().any(|wanted| match wanted {
            FindEntryType::File => entry_type == "file",
            FindEntryType::Directory => entry_type == "directory",
            FindEntryType::Symlink => entry_type == "symlink",
        })
}

fn extension_matches(path: &Path, extensions: &[String]) -> bool {
    if extensions.is_empty() {
        return true;
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    extensions.iter().any(|candidate| {
        candidate
            .trim_start_matches('.')
            .eq_ignore_ascii_case(extension)
    })
}

fn path_has_hidden_component(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .any(|value| value.starts_with('.') && value != "." && value != "..")
}

fn indexed_candidate_matches_live(
    candidate: &super::repository_index::IndexedPathCandidate,
) -> RuntimeResult<bool> {
    let metadata = match fs::symlink_metadata(&candidate.path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(super::io_error(error)),
    };
    let modified_at_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    let entry_type = metadata_kind(&metadata);
    Ok(metadata.len() == candidate.size
        && modified_at_ns == candidate.modified_at_ns
        && entry_type == candidate.entry_type)
}

fn validate_state(
    state: &FindState,
    root: &Path,
    root_version: &str,
    owner: &str,
    fingerprint: &str,
    expected_root_version: Option<&str>,
) -> RuntimeResult<()> {
    if state.root != root || state.owner != owner || state.request_fingerprint != fingerprint {
        return Err(RuntimeError::new(
            "cursor_scope_mismatch",
            "cursor does not match path, caller, or find options",
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
            "directory_changed",
            "find root changed after the cursor was issued; restart traversal",
        ));
    }
    Ok(())
}

fn root_version(path: &Path) -> RuntimeResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(super::io_error)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0_u128, |value| value.as_nanos());
    Ok(format!("{}:{modified}", metadata.len()))
}

fn owner_key(context: &OperationContext) -> String {
    format!(
        "{}:{}:{}",
        context.agent_id,
        context.task_id.as_deref().unwrap_or(""),
        context.conversation_scope_id.as_deref().unwrap_or("")
    )
}

fn file_type_kind(kind: std::fs::FileType) -> &'static str {
    if kind.is_symlink() {
        "symlink"
    } else if kind.is_dir() {
        "directory"
    } else {
        "file"
    }
}

fn metadata_kind(metadata: &fs::Metadata) -> &'static str {
    file_type_kind(metadata.file_type())
}

fn cleanup_expired(states: &mut HashMap<String, FindState>) {
    let now = Instant::now();
    states.retain(|_, state| state.expires_at > now);
}

fn evict_oldest(states: &mut HashMap<String, FindState>) {
    if let Some(key) = states
        .iter()
        .min_by_key(|(_, state)| state.expires_at)
        .map(|(key, _)| key.clone())
    {
        states.remove(&key);
    }
}

fn push_warning(warnings: &mut Vec<ToolWarning>, code: &str, message: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(ToolWarning {
            code: code.to_owned(),
            message,
        });
    }
}

#[cfg(test)]
#[path = "filesystem_find_tests.rs"]
mod tests;
