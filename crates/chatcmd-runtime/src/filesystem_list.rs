use crate::{
    FsListItemV2, FsListMetadata, FsListPageData, FsListRequestV2, FsListScanPage, FsListSort,
    OperationContext, RuntimeError, RuntimeResult, ToolWarning, TruncationReason, WorkspaceService,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, DirEntry, ReadDir},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};
use uuid::Uuid;

const LIST_STATE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ACTIVE_LIST_STATES: usize = 128;
const MAX_WARNINGS: usize = 20;

pub(super) struct DirectoryListStore {
    states: Mutex<HashMap<String, DirectoryListState>>,
}

impl Default for DirectoryListStore {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

struct DirectoryListState {
    path: PathBuf,
    directory_version: String,
    sort: FsListSort,
    metadata: Vec<FsListMetadata>,
    include_hidden: bool,
    iterator: ReadDir,
    pending_entry: Option<DirEntry>,
    expires_at: Instant,
}

impl WorkspaceService {
    pub async fn list_v2(
        &self,
        context: &OperationContext,
        request: &FsListRequestV2,
        state_id: Option<&str>,
        expected_directory_version: Option<&str>,
    ) -> RuntimeResult<(FsListScanPage, Option<String>)> {
        let resolved = self.existing(&request.path)?;
        resolved.revalidate()?;
        if !resolved.is_dir() {
            return Err(RuntimeError::new(
                "not_a_directory",
                "fs_list_v2 path must resolve to a directory",
            ));
        }

        let request = request.clone();
        let cancellation = context.cancellation.clone();
        let store = self.list_states.clone();
        let state_id = state_id.map(str::to_owned);
        let expected_directory_version = expected_directory_version.map(str::to_owned);

        tokio::task::spawn_blocking(move || {
            let current_version = directory_version(&resolved)?;
            let mut states = store.states.lock().map_err(|_| {
                RuntimeError::new("list_state_poisoned", "list state lock is poisoned")
            })?;
            cleanup_expired(&mut states);

            let (id, mut state) = if let Some(id) = state_id {
                let state = states.remove(&id).ok_or_else(|| {
                    RuntimeError::new(
                        "cursor_expired",
                        "directory cursor state is no longer available; restart listing",
                    )
                })?;
                validate_state(&state, &resolved, &request)?;
                if expected_directory_version.as_deref() != Some(state.directory_version.as_str()) {
                    return Err(RuntimeError::new(
                        "cursor_scope_mismatch",
                        "cursor directory version does not match its server state",
                    ));
                }
                if current_version != state.directory_version {
                    return Err(RuntimeError::new(
                        "directory_changed",
                        "directory changed after the cursor was issued; restart listing",
                    ));
                }
                (id, state)
            } else {
                let iterator = fs::read_dir(&resolved).map_err(super::io_error)?;
                (
                    Uuid::new_v4().to_string(),
                    DirectoryListState {
                        path: resolved.to_path_buf(),
                        directory_version: current_version,
                        sort: request.sort,
                        metadata: normalized_metadata(&request.metadata),
                        include_hidden: request.include_hidden,
                        iterator,
                        pending_entry: None,
                        expires_at: Instant::now() + LIST_STATE_TTL,
                    },
                )
            };
            drop(states);

            let page = scan_page(&mut state, &request, &cancellation)?;
            if page.has_more {
                state.expires_at = Instant::now() + LIST_STATE_TTL;
                let mut states = store.states.lock().map_err(|_| {
                    RuntimeError::new("list_state_poisoned", "list state lock is poisoned")
                })?;
                cleanup_expired(&mut states);
                if states.len() >= MAX_ACTIVE_LIST_STATES {
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
    state: &mut DirectoryListState,
    request: &FsListRequestV2,
    cancellation: &tokio_util::sync::CancellationToken,
) -> RuntimeResult<FsListScanPage> {
    let started = Instant::now();
    let limit = request.limit.clamp(1, 2_000);
    let max_entries = request.budget.max_entries_scanned.max(1);
    let max_stats = request.budget.max_stats;
    let timeout = Duration::from_millis(request.budget.timeout_ms);
    let wants = normalized_metadata(&request.metadata);
    let needs_stat =
        wants.contains(&FsListMetadata::Size) || wants.contains(&FsListMetadata::Readonly);

    let mut items = Vec::with_capacity(limit);
    let mut entries_scanned = 0_u64;
    let mut metadata_calls = 0_u64;
    let mut warnings = Vec::new();
    let mut truncation_reason = None;

    while items.len() < limit {
        if cancellation.is_cancelled() {
            truncation_reason = Some(TruncationReason::Cancelled);
            break;
        }
        if started.elapsed() >= timeout {
            truncation_reason = Some(TruncationReason::TimeBudget);
            break;
        }
        if entries_scanned >= max_entries {
            truncation_reason = Some(TruncationReason::ItemLimit);
            break;
        }

        let Some(entry) = next_entry(state, &mut warnings)? else {
            break;
        };
        entries_scanned = entries_scanned.saturating_add(1);
        if !request.include_hidden && is_hidden_name(&entry.file_name()) {
            continue;
        }
        if needs_stat && metadata_calls >= max_stats {
            state.pending_entry = Some(entry);
            truncation_reason = Some(TruncationReason::MetadataBudget);
            break;
        }
        match build_item(entry, &wants, &mut metadata_calls) {
            Ok(item) => items.push(item),
            Err(error) => push_warning(&mut warnings, &error.code, error.message),
        }
    }

    let mut has_more = truncation_reason.is_some();
    if !has_more && items.len() == limit {
        loop {
            if cancellation.is_cancelled() {
                truncation_reason = Some(TruncationReason::Cancelled);
                has_more = true;
                break;
            }
            if started.elapsed() >= timeout {
                truncation_reason = Some(TruncationReason::TimeBudget);
                has_more = true;
                break;
            }
            if entries_scanned >= max_entries {
                truncation_reason = Some(TruncationReason::ItemLimit);
                has_more = true;
                break;
            }
            let Some(entry) = next_entry(state, &mut warnings)? else {
                break;
            };
            entries_scanned = entries_scanned.saturating_add(1);
            if !request.include_hidden && is_hidden_name(&entry.file_name()) {
                continue;
            }
            state.pending_entry = Some(entry);
            has_more = true;
            break;
        }
    }

    Ok(FsListScanPage {
        data: FsListPageData {
            items,
            directory_version: state.directory_version.clone(),
            sort: request.sort,
        },
        has_more,
        entries_scanned,
        metadata_calls,
        truncation_reason,
        warnings,
    })
}

fn next_entry(
    state: &mut DirectoryListState,
    warnings: &mut Vec<ToolWarning>,
) -> RuntimeResult<Option<DirEntry>> {
    if let Some(entry) = state.pending_entry.take() {
        return Ok(Some(entry));
    }
    loop {
        match state.iterator.next() {
            Some(Ok(entry)) => return Ok(Some(entry)),
            Some(Err(error)) => push_warning(
                warnings,
                "entry_unavailable",
                format!("directory entry could not be read: {error}"),
            ),
            None => return Ok(None),
        }
    }
}

fn validate_state(
    state: &DirectoryListState,
    resolved: &Path,
    request: &FsListRequestV2,
) -> RuntimeResult<()> {
    if state.path != resolved
        || state.sort != request.sort
        || state.metadata != normalized_metadata(&request.metadata)
        || state.include_hidden != request.include_hidden
    {
        return Err(RuntimeError::new(
            "cursor_scope_mismatch",
            "cursor does not match path, sort, metadata, or filter options",
        ));
    }
    Ok(())
}

fn normalized_metadata(metadata: &[FsListMetadata]) -> Vec<FsListMetadata> {
    let mut result = Vec::with_capacity(metadata.len());
    for field in [
        FsListMetadata::Type,
        FsListMetadata::Size,
        FsListMetadata::Readonly,
    ] {
        if metadata.contains(&field) {
            result.push(field);
        }
    }
    result
}

fn build_item(
    entry: DirEntry,
    wants: &[FsListMetadata],
    metadata_calls: &mut u64,
) -> RuntimeResult<FsListItemV2> {
    let file_name = entry.file_name();
    let name_encoding_lossy = file_name.to_str().is_none();
    let name = file_name.to_string_lossy().into_owned();
    let path = entry.path();
    let path_text = path.to_string_lossy().into_owned();
    let needs_stat =
        wants.contains(&FsListMetadata::Size) || wants.contains(&FsListMetadata::Readonly);
    let metadata = if needs_stat {
        *metadata_calls = metadata_calls.saturating_add(1);
        Some(fs::symlink_metadata(&path).map_err(|error| {
            RuntimeError::new(
                "entry_metadata_unavailable",
                format!("metadata unavailable for {}: {error}", path.display()),
            )
        })?)
    } else {
        None
    };

    let entry_type = if wants.contains(&FsListMetadata::Type) {
        let file_type = if let Some(metadata) = metadata.as_ref() {
            metadata.file_type()
        } else {
            entry.file_type().map_err(|error| {
                RuntimeError::new(
                    "entry_metadata_unavailable",
                    format!("type unavailable for {}: {error}", path.display()),
                )
            })?
        };
        Some(
            if file_type.is_symlink() {
                "symlink"
            } else if file_type.is_dir() {
                "directory"
            } else if file_type.is_file() {
                "file"
            } else {
                "other"
            }
            .to_owned(),
        )
    } else {
        None
    };

    Ok(FsListItemV2 {
        name,
        path: path_text,
        entry_type,
        size: wants
            .contains(&FsListMetadata::Size)
            .then(|| metadata.as_ref().expect("metadata requested").len()),
        readonly: wants.contains(&FsListMetadata::Readonly).then(|| {
            metadata
                .as_ref()
                .expect("metadata requested")
                .permissions()
                .readonly()
        }),
        name_encoding_lossy,
    })
}

fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn directory_version(path: &Path) -> RuntimeResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(super::io_error)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    let created = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(path.to_string_lossy().as_bytes());
    hash.update(modified.as_nanos().to_le_bytes());
    hash.update(created.as_nanos().to_le_bytes());
    hash.update(metadata.len().to_le_bytes());
    hash.update([u8::from(metadata.permissions().readonly())]);
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn cleanup_expired(states: &mut HashMap<String, DirectoryListState>) {
    let now = Instant::now();
    states.retain(|_, state| state.expires_at > now);
}

fn evict_oldest(states: &mut HashMap<String, DirectoryListState>) {
    if let Some(id) = states
        .iter()
        .min_by_key(|(_, state)| state.expires_at)
        .map(|(id, _)| id.clone())
    {
        states.remove(&id);
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
mod tests {
    use super::*;
    use crate::{ApprovalDecision, BoxFuture, FsListBudget, PolicyContext, PolicyEngine};
    use std::{fs::File, sync::Arc};
    use tempfile::tempdir;

    struct Reject;

    impl ApprovalDecision for Reject {
        fn request<'a>(
            &'a self,
            _context: &'a PolicyContext,
        ) -> BoxFuture<'a, RuntimeResult<bool>> {
            Box::pin(async { Ok(false) })
        }
    }

    fn service(root: &Path) -> WorkspaceService {
        WorkspaceService::new(
            &[root.to_path_buf()],
            PolicyEngine::new(None, Arc::new(Reject)),
        )
        .expect("workspace")
    }

    fn request(path: &Path, limit: usize) -> FsListRequestV2 {
        FsListRequestV2 {
            path: path.to_path_buf(),
            limit,
            sort: FsListSort::Filesystem,
            metadata: Vec::new(),
            include_hidden: true,
            budget: FsListBudget {
                timeout_ms: 10_000,
                max_entries_scanned: 20_000,
                max_stats: 20_000,
            },
        }
    }

    #[tokio::test]
    async fn page_boundaries_cover_zero_one_limit_and_limit_plus_one() {
        for count in [0_usize, 1, 5, 6] {
            let temp = tempdir().expect("tempdir");
            for index in 0..count {
                File::create(temp.path().join(format!("file-{index}.txt"))).expect("create");
            }
            let workspace = service(temp.path());
            let context = OperationContext::new("r", "a", "fs_list_v2");
            let (page, state) = workspace
                .list_v2(&context, &request(temp.path(), 5), None, None)
                .await
                .expect("page");
            assert_eq!(page.data.items.len(), count.min(5));
            assert_eq!(page.has_more, count > 5);
            assert_eq!(state.is_some(), count > 5);
        }
    }

    #[tokio::test]
    async fn paginates_large_directory_without_materializing_all_entries() {
        let temp = tempdir().expect("tempdir");
        for index in 0..10_001 {
            File::create(temp.path().join(format!("file-{index:05}.txt"))).expect("create");
        }
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let (page, state) = workspace
            .list_v2(&context, &request(temp.path(), 25), None, None)
            .await
            .expect("first page");
        assert_eq!(page.data.items.len(), 25);
        assert!(page.has_more);
        assert!(page.entries_scanned <= 26);
        assert_eq!(page.metadata_calls, 0);
        assert!(state.is_some());
    }

    #[tokio::test]
    async fn stable_directory_pages_have_no_duplicates_or_omissions() {
        let temp = tempdir().expect("tempdir");
        for index in 0..257 {
            File::create(temp.path().join(format!("file-{index:03}.txt"))).expect("create");
        }
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let request = request(temp.path(), 31);
        let mut state_id = None;
        let mut version = None;
        let mut names = Vec::new();
        loop {
            let (page, next_state) = workspace
                .list_v2(&context, &request, state_id.as_deref(), version.as_deref())
                .await
                .expect("page");
            version = Some(page.data.directory_version.clone());
            names.extend(page.data.items.into_iter().map(|item| item.name));
            if !page.has_more {
                break;
            }
            state_id = next_state;
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 257);
    }

    #[tokio::test]
    async fn metadata_empty_performs_no_metadata_calls() {
        let temp = tempdir().expect("tempdir");
        File::create(temp.path().join("one.txt")).expect("create");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let (page, _) = workspace
            .list_v2(&context, &request(temp.path(), 10), None, None)
            .await
            .expect("page");
        assert_eq!(page.metadata_calls, 0);
        assert!(page.data.items[0].entry_type.is_none());
        assert!(page.data.items[0].size.is_none());
    }

    #[tokio::test]
    async fn cursor_is_bound_to_filter_and_metadata_options() {
        let temp = tempdir().expect("tempdir");
        File::create(temp.path().join("a.txt")).expect("create");
        File::create(temp.path().join("b.txt")).expect("create");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let first_request = request(temp.path(), 1);
        let (page, state) = workspace
            .list_v2(&context, &first_request, None, None)
            .await
            .expect("first");
        let mut changed = first_request;
        changed.include_hidden = false;
        let error = workspace
            .list_v2(
                &context,
                &changed,
                state.as_deref(),
                Some(&page.data.directory_version),
            )
            .await
            .expect_err("option mismatch");
        assert_eq!(error.code, "cursor_scope_mismatch");
    }

    #[tokio::test]
    async fn cursor_state_rejects_a_different_directory_path() {
        let temp = tempdir().expect("tempdir");
        let first_dir = temp.path().join("first");
        let second_dir = temp.path().join("second");
        fs::create_dir_all(&first_dir).expect("first directory");
        fs::create_dir_all(&second_dir).expect("second directory");
        File::create(first_dir.join("a.txt")).expect("create first file");
        File::create(first_dir.join("b.txt")).expect("create second first file");
        File::create(second_dir.join("other.txt")).expect("create second file");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let first_request = request(&first_dir, 1);
        let (page, state) = workspace
            .list_v2(&context, &first_request, None, None)
            .await
            .expect("first page");
        let error = workspace
            .list_v2(
                &context,
                &request(&second_dir, 1),
                state.as_deref(),
                Some(&page.data.directory_version),
            )
            .await
            .expect_err("path mismatch must fail");
        assert_eq!(error.code, "cursor_scope_mismatch");
    }

    #[tokio::test]
    async fn case_distinct_names_match_the_underlying_filesystem_view() {
        let temp = tempdir().expect("tempdir");
        File::create(temp.path().join("Case.txt")).expect("create first case variant");
        File::create(temp.path().join("case.txt")).expect("create second case variant");
        let mut expected = fs::read_dir(temp.path())
            .expect("read directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        expected.sort();

        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let (page, _) = workspace
            .list_v2(&context, &request(temp.path(), 10), None, None)
            .await
            .expect("page");
        let mut actual = page
            .data
            .items
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_names_are_returned_with_an_explicit_lossy_marker() {
        use std::os::unix::ffi::OsStringExt as _;

        let temp = tempdir().expect("tempdir");
        let invalid_name = std::ffi::OsString::from_vec(b"invalid-\xff.txt".to_vec());
        File::create(temp.path().join(invalid_name)).expect("create non-utf8 file");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let (page, _) = workspace
            .list_v2(&context, &request(temp.path(), 10), None, None)
            .await
            .expect("page");
        assert_eq!(page.data.items.len(), 1);
        assert!(page.data.items[0].name_encoding_lossy);
        assert!(page.data.items[0].name.contains('\u{fffd}'));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_symlink_and_socket_entries_are_reported_without_following_targets() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let temp = tempdir().expect("tempdir");
        symlink("missing-target", temp.path().join("broken-link")).expect("create symlink");
        let _socket = UnixListener::bind(temp.path().join("local.sock")).expect("bind unix socket");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let mut list_request = request(temp.path(), 10);
        list_request.metadata = vec![FsListMetadata::Type];
        let (page, _) = workspace
            .list_v2(&context, &list_request, None, None)
            .await
            .expect("page");
        let types = page
            .data
            .items
            .into_iter()
            .map(|item| (item.name, item.entry_type))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            types.get("broken-link").and_then(Option::as_deref),
            Some("symlink")
        );
        assert_eq!(
            types.get("local.sock").and_then(Option::as_deref),
            Some("other")
        );
    }

    #[test]
    fn entry_removed_before_requested_metadata_becomes_a_nonfatal_entry_error() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("vanishing.txt");
        File::create(&path).expect("create file");
        let entry = fs::read_dir(temp.path())
            .expect("read directory")
            .next()
            .expect("entry exists")
            .expect("read entry");
        fs::remove_file(&path).expect("remove file before metadata");
        let mut metadata_calls = 0;
        let error = build_item(entry, &[FsListMetadata::Size], &mut metadata_calls)
            .expect_err("metadata race should be recoverable by caller");
        assert_eq!(error.code, "entry_metadata_unavailable");
        assert_eq!(metadata_calls, 1);
    }

    #[tokio::test]
    async fn expired_server_cursor_state_requires_restart() {
        let temp = tempdir().expect("tempdir");
        File::create(temp.path().join("a.txt")).expect("create");
        File::create(temp.path().join("b.txt")).expect("create");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let request = request(temp.path(), 1);
        let (page, state) = workspace
            .list_v2(&context, &request, None, None)
            .await
            .expect("first");
        let state_id = state.expect("continuation state");
        workspace
            .list_states
            .states
            .lock()
            .expect("state lock")
            .get_mut(&state_id)
            .expect("stored state")
            .expires_at = Instant::now() - Duration::from_millis(1);

        let error = workspace
            .list_v2(
                &context,
                &request,
                Some(&state_id),
                Some(&page.data.directory_version),
            )
            .await
            .expect_err("expired state must fail");
        assert_eq!(error.code, "cursor_expired");
    }

    #[tokio::test]
    async fn metadata_budget_can_resume_with_a_larger_budget() {
        let temp = tempdir().expect("tempdir");
        File::create(temp.path().join("a.txt")).expect("create");
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let mut request = request(temp.path(), 1);
        request.metadata = vec![FsListMetadata::Size];
        request.budget.max_stats = 0;
        let (page, state) = workspace
            .list_v2(&context, &request, None, None)
            .await
            .expect("budget-limited page");
        assert!(page.data.items.is_empty());
        assert_eq!(
            page.truncation_reason,
            Some(TruncationReason::MetadataBudget)
        );
        assert_eq!(page.metadata_calls, 0);

        request.budget.max_stats = 1;
        let (page, _) = workspace
            .list_v2(
                &context,
                &request,
                state.as_deref(),
                Some(&page.data.directory_version),
            )
            .await
            .expect("resumed page");
        assert_eq!(page.data.items.len(), 1);
        assert_eq!(page.metadata_calls, 1);
    }

    #[tokio::test]
    async fn rejects_changed_directory_on_continuation() {
        let temp = tempdir().expect("tempdir");
        for index in 0..3 {
            File::create(temp.path().join(format!("file-{index}.txt"))).expect("create");
        }
        let workspace = service(temp.path());
        let context = OperationContext::new("r", "a", "fs_list_v2");
        let request = request(temp.path(), 1);
        let (page, state) = workspace
            .list_v2(&context, &request, None, None)
            .await
            .expect("first");
        std::thread::sleep(Duration::from_millis(25));
        File::create(temp.path().join("changed.txt")).expect("create");
        let error = workspace
            .list_v2(
                &context,
                &request,
                state.as_deref(),
                Some(&page.data.directory_version),
            )
            .await
            .expect_err("directory change must invalidate cursor");
        assert_eq!(error.code, "directory_changed");
    }

    #[tokio::test]
    async fn legacy_list_keeps_sorted_offset_limit_contract() {
        let temp = tempdir().expect("tempdir");
        for name in ["c.txt", "a.txt", "b.txt"] {
            File::create(temp.path().join(name)).expect("create");
        }
        let workspace = service(temp.path());
        let entries = workspace
            .list(temp.path(), 1, 1)
            .await
            .expect("legacy list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "b.txt");
    }

    #[tokio::test]
    async fn cancellation_and_time_budget_stop_traversal() {
        let temp = tempdir().expect("tempdir");
        for index in 0..50 {
            File::create(temp.path().join(format!("file-{index}.txt"))).expect("create");
        }
        let workspace = service(temp.path());
        let cancelled = OperationContext::new("r", "a", "fs_list_v2");
        cancelled.cancellation.cancel();
        let (page, state) = workspace
            .list_v2(&cancelled, &request(temp.path(), 10), None, None)
            .await
            .expect("cancelled page");
        assert_eq!(page.truncation_reason, Some(TruncationReason::Cancelled));
        assert!(state.is_some());

        let context = OperationContext::new("r2", "a", "fs_list_v2");
        let mut timed = request(temp.path(), 10);
        timed.budget.timeout_ms = 0;
        let (page, state) = workspace
            .list_v2(&context, &timed, None, None)
            .await
            .expect("timed page");
        assert_eq!(page.truncation_reason, Some(TruncationReason::TimeBudget));
        assert!(state.is_some());
    }

    #[tokio::test]
    #[ignore = "manual large-directory benchmark"]
    async fn benchmark_first_and_next_page() {
        let temp = tempdir().expect("tempdir");
        for index in 0..100_000 {
            File::create(temp.path().join(format!("file-{index:06}.txt"))).expect("create");
        }
        let workspace = service(temp.path());
        let context = OperationContext::new("bench", "a", "fs_list_v2");
        let request = request(temp.path(), 200);
        let first_started = Instant::now();
        let (first, state) = workspace
            .list_v2(&context, &request, None, None)
            .await
            .expect("first");
        let first_elapsed = first_started.elapsed();
        let second_started = Instant::now();
        let (second, _) = workspace
            .list_v2(
                &context,
                &request,
                state.as_deref(),
                Some(&first.data.directory_version),
            )
            .await
            .expect("second");
        println!(
            "fs_list_v2 benchmark entries=100000 limit=200 first_us={} next_us={} first_scanned={} next_scanned={}",
            first_elapsed.as_micros(),
            second_started.elapsed().as_micros(),
            first.entries_scanned,
            second.entries_scanned
        );
    }
}
