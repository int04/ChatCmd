use super::{
    WorkspaceService,
    search_helpers::{
        append_after_context, compile_search, drain_ready, flush_pending, include_matches,
        open_text_file, truncate_utf8,
    },
    search_state::{
        cleanup_expired, evict_oldest, has_next_file, next_file_entry, owner_key, push_warning,
        root_version, validate_state,
    },
    walk::configured_walker,
};
use crate::{
    BudgetTracker, FsSearchMatch, FsSearchPageData, FsSearchRequest, FsSearchScanPage,
    OperationContext, ProgressLimiter, RuntimeError, RuntimeResult, ToolBudget, TraversalOptions,
    TruncationReason,
};
use globset::GlobSet;
use ignore::{DirEntry, Walk};
use regex::Regex;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

const SEARCH_STATE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ACTIVE_SEARCH_STATES: usize = 128;
const HARD_SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
const HARD_SEARCH_FILES: u64 = 1_000_000;
const HARD_SEARCH_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const HARD_SEARCH_OUTPUT: u64 = 4 * 1024 * 1024;
const HARD_SEARCH_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub path: PathBuf,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub matches_found: usize,
}

pub(super) struct SearchStateStore {
    states: Mutex<HashMap<String, SearchState>>,
}

impl Default for SearchStateStore {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

pub(super) struct SearchState {
    pub(super) root: PathBuf,
    pub(super) root_version: String,
    pub(super) owner: String,
    pub(super) request_fingerprint: String,
    pub(super) walker: Walk,
    pub(super) pending_entry: Option<DirEntry>,
    current_file: Option<FileScanState>,
    pub(super) expires_at: Instant,
}

struct FileScanState {
    path: PathBuf,
    reader: BufReader<File>,
    line_number: u64,
    byte_offset: u64,
    context_before: VecDeque<String>,
    pending: Vec<PendingMatch>,
    ready: VecDeque<FsSearchMatch>,
    matches_in_file: usize,
    invalid_utf8_reported: bool,
}

pub(super) struct PendingMatch {
    pub(super) value: FsSearchMatch,
    pub(super) after_remaining: usize,
}

pub(super) struct CompiledSearch {
    pub(super) regex: Regex,
    pub(super) includes: Option<GlobSet>,
}

impl WorkspaceService {
    pub async fn search_v2(
        &self,
        context: &OperationContext,
        request: &FsSearchRequest,
        state_id: Option<&str>,
        expected_root_version: Option<&str>,
        progress: impl Fn(SearchProgress) + Send + Sync + 'static,
    ) -> RuntimeResult<(FsSearchScanPage, Option<String>)> {
        let memory = request.budget.max_output_bytes.min(64 * 1024 * 1024);
        let _admission = self.admission.try_admit(&context.agent_id, 1, memory)?;
        let root = self.existing(&request.path)?;
        root.revalidate()?;
        let mut request = request.clone();
        request.budget.timeout_ms = request.budget.timeout_ms.min(60_000);
        request.budget.max_files_scanned =
            request.budget.max_files_scanned.clamp(1, HARD_SEARCH_FILES);
        request.budget.max_bytes_scanned =
            request.budget.max_bytes_scanned.clamp(1, HARD_SEARCH_BYTES);
        request.budget.max_output_bytes =
            request.budget.max_output_bytes.clamp(1, HARD_SEARCH_OUTPUT);
        request.budget.max_file_bytes = request
            .budget
            .max_file_bytes
            .clamp(1, HARD_SEARCH_FILE_BYTES);
        let tracker = BudgetTracker::new(
            context.cancellation.clone(),
            ToolBudget::intersect([
                &ToolBudget {
                    deadline: Some(Instant::now() + HARD_SEARCH_TIMEOUT),
                    ..ToolBudget::default()
                },
                &ToolBudget {
                    deadline: Some(
                        Instant::now() + Duration::from_millis(request.budget.timeout_ms),
                    ),
                    ..ToolBudget::default()
                },
            ]),
        );
        let owner = owner_key(context);
        let store = self.search_states.clone();
        let state_id = state_id.map(str::to_owned);
        let expected_root_version = expected_root_version.map(str::to_owned);

        tokio::task::spawn_blocking(move || {
            let compiled = compile_search(&request)?;
            let version = root_version(&root)?;
            let fingerprint = serde_json::to_string(&request)
                .map_err(|error| RuntimeError::new("search_request_invalid", error.to_string()))?;
            let mut states = store.states.lock().map_err(|_| {
                RuntimeError::new("search_state_poisoned", "search state lock is poisoned")
            })?;
            cleanup_expired(&mut states);
            let (id, mut state) = if let Some(id) = state_id {
                let state = states.remove(&id).ok_or_else(|| {
                    RuntimeError::new(
                        "cursor_expired",
                        "search cursor state expired; restart search",
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
                        include_hidden: true,
                        include_ignored: request.include_ignored,
                        exclude: request.exclude.clone(),
                        ..TraversalOptions::default()
                    },
                )?
                .build();
                (
                    Uuid::new_v4().to_string(),
                    SearchState {
                        root: root.to_path_buf(),
                        root_version: version.clone(),
                        owner,
                        request_fingerprint: fingerprint,
                        walker,
                        pending_entry: None,
                        current_file: None,
                        expires_at: Instant::now() + SEARCH_STATE_TTL,
                    },
                )
            };
            drop(states);

            let page = scan_page(&mut state, &request, &compiled, &tracker, &progress)?;
            if page.has_more {
                state.expires_at = Instant::now() + SEARCH_STATE_TTL;
                let mut states = store.states.lock().map_err(|_| {
                    RuntimeError::new("search_state_poisoned", "search state lock is poisoned")
                })?;
                cleanup_expired(&mut states);
                if states.len() >= MAX_ACTIVE_SEARCH_STATES {
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
    state: &mut SearchState,
    request: &FsSearchRequest,
    compiled: &CompiledSearch,
    tracker: &BudgetTracker,
    progress: &impl Fn(SearchProgress),
) -> RuntimeResult<FsSearchScanPage> {
    let limit = request.limit.clamp(1, 5_000);
    let max_files = request.budget.max_files_scanned.clamp(1, HARD_SEARCH_FILES);
    let max_bytes = request.budget.max_bytes_scanned.clamp(1, HARD_SEARCH_BYTES);
    let max_output = request.budget.max_output_bytes.clamp(1, HARD_SEARCH_OUTPUT);
    let mut matches = Vec::with_capacity(limit.min(256));
    let mut files_scanned = 0_u64;
    let mut bytes_scanned = 0_u64;
    let mut output_bytes = 0_u64;
    let mut files_skipped_by_size = 0_u64;
    let mut binary_files_skipped = 0_u64;
    let mut errors_skipped = 0_u64;
    let mut warnings = Vec::new();
    let mut truncation_reason = None;
    let progress_limiter = ProgressLimiter::new(10, 100);

    loop {
        if let Err(error) = tracker.checkpoint() {
            truncation_reason = Some(if error.code == "operationCancelled" {
                TruncationReason::Cancelled
            } else {
                TruncationReason::TimeBudget
            });
            break;
        }
        if files_scanned >= max_files && state.current_file.is_none() {
            truncation_reason = Some(TruncationReason::FileBudget);
            break;
        }
        if bytes_scanned >= max_bytes {
            truncation_reason = Some(TruncationReason::ByteBudget);
            break;
        }
        if matches.len() >= limit {
            if let Some(file) = state.current_file.as_mut() {
                if file.ready.is_empty()
                    && file.pending.is_empty()
                    && file.reader.fill_buf().map_err(super::io_error)?.is_empty()
                {
                    state.current_file = None;
                }
            }
            if state.current_file.is_some() || has_next_file(state, &mut warnings)? {
                truncation_reason = Some(TruncationReason::ItemLimit);
            }
            break;
        }
        if let Some(file) = state.current_file.as_mut() {
            if !drain_ready(
                &mut file.ready,
                &mut matches,
                &mut output_bytes,
                max_output,
                limit,
            )? {
                truncation_reason = Some(if matches.len() >= limit {
                    TruncationReason::ItemLimit
                } else {
                    TruncationReason::OutputLimit
                });
                break;
            }
        }

        if state.current_file.is_none() {
            let Some(entry) = next_file_entry(state, &mut warnings)? else {
                break;
            };
            if !include_matches(&state.root, &entry, compiled.includes.as_ref()) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(value) => value,
                Err(error) => {
                    errors_skipped += 1;
                    push_warning(
                        &mut warnings,
                        "filesystem_metadata_error",
                        error.to_string(),
                    );
                    continue;
                }
            };
            if metadata.len() > request.budget.max_file_bytes {
                files_skipped_by_size += 1;
                continue;
            }
            match open_text_file(entry.path()) {
                Ok(Some(file)) => {
                    files_scanned += 1;
                    tracker.record_files(1);
                    state.current_file = Some(FileScanState {
                        path: entry.path().to_path_buf(),
                        reader: BufReader::with_capacity(64 * 1024, file),
                        line_number: 0,
                        byte_offset: 0,
                        context_before: VecDeque::with_capacity(request.context_before.min(100)),
                        pending: Vec::new(),
                        ready: VecDeque::new(),
                        matches_in_file: 0,
                        invalid_utf8_reported: false,
                    });
                }
                Ok(None) => {
                    binary_files_skipped += 1;
                    continue;
                }
                Err(error) => {
                    errors_skipped += 1;
                    push_warning(&mut warnings, "filesystem_read_error", error.to_string());
                    continue;
                }
            }
        }

        let file = state.current_file.as_mut().expect("file initialized above");
        let mut raw = Vec::new();
        let read = match file.reader.read_until(b'\n', &mut raw) {
            Ok(value) => value,
            Err(error) => {
                errors_skipped += 1;
                push_warning(&mut warnings, "filesystem_read_error", error.to_string());
                state.current_file = None;
                continue;
            }
        };
        if read == 0 {
            flush_pending(&mut file.pending, &mut file.ready, true);
            if !drain_ready(
                &mut file.ready,
                &mut matches,
                &mut output_bytes,
                max_output,
                limit,
            )? {
                truncation_reason = Some(if matches.len() >= limit {
                    TruncationReason::ItemLimit
                } else {
                    TruncationReason::OutputLimit
                });
                break;
            }
            state.current_file = None;
            continue;
        }
        bytes_scanned = bytes_scanned.saturating_add(read as u64);
        tracker.record_read_bytes(read as u64);
        file.line_number += 1;
        let line_start = file.byte_offset;
        file.byte_offset = file.byte_offset.saturating_add(read as u64);

        let had_newline = raw.last() == Some(&b'\n');
        if had_newline {
            raw.pop();
        }
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        let bom_skip = if file.line_number == 1 && raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        let line_bytes = &raw[bom_skip..];
        let line = match std::str::from_utf8(line_bytes) {
            Ok(value) => value.to_owned(),
            Err(_) => {
                if !file.invalid_utf8_reported {
                    errors_skipped += 1;
                    push_warning(
                        &mut warnings,
                        "invalid_utf8_lossy",
                        format!(
                            "{} contains invalid UTF-8; replacement characters were used",
                            file.path.display()
                        ),
                    );
                    file.invalid_utf8_reported = true;
                }
                String::from_utf8_lossy(line_bytes).into_owned()
            }
        };

        let (context_line, _) = truncate_utf8(&line, request.max_snippet_bytes.max(64));
        append_after_context(&mut file.pending, &context_line);
        flush_pending(&mut file.pending, &mut file.ready, false);
        if !drain_ready(
            &mut file.ready,
            &mut matches,
            &mut output_bytes,
            max_output,
            limit,
        )? {
            truncation_reason = Some(if matches.len() >= limit {
                TruncationReason::ItemLimit
            } else {
                TruncationReason::OutputLimit
            });
            break;
        }

        if file.matches_in_file < request.max_matches_per_file.max(1) {
            for found in compiled.regex.find_iter(&line) {
                if file.matches_in_file >= request.max_matches_per_file.max(1) {
                    break;
                }
                let (line_text, line_truncated) =
                    truncate_utf8(&line, request.max_snippet_bytes.max(64));
                let match_text = line[found.start()..found.end()].to_owned();
                let value = FsSearchMatch {
                    path: file.path.to_string_lossy().into_owned(),
                    line: file.line_number,
                    column: line[..found.start()].chars().count() as u64 + 1,
                    byte_offset: line_start + bom_skip as u64 + found.start() as u64,
                    match_start: found.start() as u64,
                    match_end: found.end() as u64,
                    match_text,
                    line_text,
                    context_before: file.context_before.iter().cloned().collect(),
                    context_after: Vec::new(),
                    line_truncated,
                };
                file.matches_in_file += 1;
                if request.context_after == 0 {
                    file.ready.push_back(value);
                } else {
                    file.pending.push(PendingMatch {
                        value,
                        after_remaining: request.context_after.min(100),
                    });
                }
            }
        }
        if !drain_ready(
            &mut file.ready,
            &mut matches,
            &mut output_bytes,
            max_output,
            limit,
        )? {
            truncation_reason = Some(if matches.len() >= limit {
                TruncationReason::ItemLimit
            } else {
                TruncationReason::OutputLimit
            });
            break;
        }

        if request.context_before > 0 {
            let (snippet, _) = truncate_utf8(&line, request.max_snippet_bytes.max(64));
            file.context_before.push_back(snippet);
            while file.context_before.len() > request.context_before.min(100) {
                file.context_before.pop_front();
            }
        }

        if progress_limiter.should_emit(false) {
            progress(SearchProgress {
                path: file.path.clone(),
                files_scanned,
                bytes_scanned,
                matches_found: matches.len() + file.pending.len(),
            });
        }
    }

    if progress_limiter.should_emit(true) {
        progress(SearchProgress {
            path: state
                .current_file
                .as_ref()
                .map_or_else(|| state.root.clone(), |file| file.path.clone()),
            files_scanned,
            bytes_scanned,
            matches_found: matches.len(),
        });
    }

    let has_more = truncation_reason.is_some()
        || state.current_file.is_some()
        || has_next_file(state, &mut warnings)?;
    Ok(FsSearchScanPage {
        data: FsSearchPageData {
            matches,
            files_skipped_by_size,
            binary_files_skipped,
            errors_skipped,
        },
        has_more,
        files_scanned,
        bytes_scanned,
        truncation_reason,
        warnings,
        root_version: state.root_version.clone(),
    })
}
