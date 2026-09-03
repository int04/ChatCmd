use super::{WorkspaceService, walk::configured_walker};
use crate::{
    BudgetTracker, FindEntryType, FindPatternMode, FsFindItem, FsFindPageData, FsFindRequest,
    FsFindScanPage, OperationContext, RuntimeError, RuntimeResult, ToolBudget, ToolWarning,
    TraversalOptions, TruncationReason,
};
use globset::{GlobBuilder, GlobMatcher};
use ignore::{DirEntry, Walk};
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
    walker: Walk,
    pending: Option<DirEntry>,
    expires_at: Instant,
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
                (id, state)
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
                    Uuid::new_v4().to_string(),
                    FindState {
                        root: root.to_path_buf(),
                        root_version: version.clone(),
                        owner,
                        request_fingerprint: fingerprint,
                        walker,
                        pending: None,
                        expires_at: Instant::now() + FIND_STATE_TTL,
                    },
                )
            };
            drop(states);

            let page = scan_page(&mut state, &request, &matcher, &tracker)?;
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
        if entry.depth() == 0 && entry.path() == state.root {
            if state.root.is_dir() {
                continue;
            }
        }
        if !entry_type_matches(&entry, &request.entry_types) {
            continue;
        }
        if !extension_matches(&entry, &request.extensions) {
            continue;
        }
        if !pattern_matches(&state.root, &entry, matcher, request.case_sensitive) {
            continue;
        }
        let Some(kind) = entry.file_type() else {
            if metadata_calls >= max_metadata {
                state.pending = Some(entry);
                truncation_reason = Some(TruncationReason::MetadataBudget);
                break;
            }
            metadata_calls = metadata_calls.saturating_add(1);
            let metadata = fs::symlink_metadata(entry.path()).map_err(super::io_error)?;
            items.push(FsFindItem {
                path: entry.path().to_string_lossy().into_owned(),
                entry_type: metadata_kind(&metadata).to_owned(),
            });
            continue;
        };
        items.push(FsFindItem {
            path: entry.path().to_string_lossy().into_owned(),
            entry_type: file_type_kind(kind).to_owned(),
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
        data: FsFindPageData { items },
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
) -> RuntimeResult<Option<DirEntry>> {
    if let Some(entry) = state.pending.take() {
        return Ok(Some(entry));
    }
    loop {
        match state.walker.next() {
            Some(Ok(entry)) => return Ok(Some(entry)),
            Some(Err(error)) => push_warning(warnings, "filesystem_walk_error", error.to_string()),
            None => return Ok(None),
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
    entry: &DirEntry,
    matcher: &CompiledPattern,
    case_sensitive: bool,
) -> bool {
    let relative = entry
        .path()
        .strip_prefix(root)
        .unwrap_or_else(|_| entry.path());
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    match matcher {
        CompiledPattern::Literal(needle) => {
            let name = entry.file_name().to_string_lossy();
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

fn entry_type_matches(entry: &DirEntry, wanted: &[FindEntryType]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    entry.file_type().is_some_and(|kind| {
        wanted.iter().any(|entry_type| match entry_type {
            FindEntryType::File => kind.is_file(),
            FindEntryType::Directory => kind.is_dir(),
            FindEntryType::Symlink => kind.is_symlink(),
        })
    })
}

fn extension_matches(entry: &DirEntry, extensions: &[String]) -> bool {
    if extensions.is_empty() {
        return true;
    }
    let Some(extension) = entry.path().extension().and_then(|value| value.to_str()) else {
        return false;
    };
    extensions.iter().any(|candidate| {
        candidate
            .trim_start_matches('.')
            .eq_ignore_ascii_case(extension)
    })
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
