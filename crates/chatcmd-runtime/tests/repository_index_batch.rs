use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, FindPatternMode, FsBatchReadRequest, FsBatchStatBudget,
    FsBatchStatRequest, FsFindRequest, FsSearchRequest, OperationContext, PolicyContext,
    PolicyEngine, RepositoryIndexEntrySnapshot, RepositoryIndexSnapshot, RuntimeResult, SearchMode,
    TextReadBudget, TextReadRange, TextReadRequestV2, VersionStrength, WorkspaceService,
};
use std::{path::Path, sync::Arc};

struct Reject;
impl ApprovalDecision for Reject {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
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

#[tokio::test]
async fn rebuild_publishes_generation_and_mutation_marks_stale() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("one.txt"), "one").expect("write");
    let workspace = service(temp.path());
    let context = OperationContext::new("index", "agent", "workspace_index_rebuild");

    let first = workspace
        .rebuild_index(&context, temp.path())
        .await
        .expect("rebuild");
    assert_eq!(first.generation, 1);
    assert!(first.entry_count >= 2);

    workspace.mark_index_stale(&temp.path().join("one.txt"));
    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
    let second = workspace
        .rebuild_index(&context, temp.path())
        .await
        .expect("rebuild again");
    assert_eq!(second.generation, 3);
}

#[tokio::test]
async fn batches_preserve_order_errors_duplicates_and_output_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = temp.path().join("first.txt");
    let second = temp.path().join("second.txt");
    std::fs::write(&first, "first").expect("write first");
    std::fs::write(&second, "second").expect("write second");
    let missing = temp.path().join("missing.txt");
    let workspace = service(temp.path());
    let context = OperationContext::new("batch", "agent", "fs_batch_stat");

    let stats = workspace
        .batch_stat(
            &context,
            &FsBatchStatRequest {
                paths: vec![first.clone(), missing.clone(), first.clone()],
                version_strength: Default::default(),
                max_items: 3,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch stat");
    assert_eq!(
        stats
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>(),
        vec![first.clone(), missing, first.clone()]
    );
    assert_eq!((stats.usage.succeeded, stats.usage.failed), (2, 1));

    let read = |path| TextReadRequestV2 {
        path,
        range: TextReadRange::Byte {
            start: 0,
            limit: 32,
        },
        max_bytes: 32,
        include_line_endings: true,
        expected_version: None,
        budget: Default::default(),
    };
    let reads = workspace
        .batch_read(
            &context,
            &FsBatchReadRequest {
                requests: vec![read(first), read(second)],
                max_items: 2,
                max_total_output_bytes: 5,
                concurrency: 2,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch read");
    assert_eq!(reads.items.len(), 2);
    assert!(reads.items[0].ok);
    assert!(!reads.items[1].ok);
    assert!(reads.truncated);
}

#[tokio::test]
async fn fresh_index_accelerates_find_search_and_batch_stat_then_stale_falls_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let alpha = temp.path().join("alpha.txt");
    let beta = temp.path().join("beta.rs");
    std::fs::write(&alpha, "needle alpha\n").expect("write alpha");
    std::fs::write(&beta, "needle beta\n").expect("write beta");
    let workspace = service(temp.path());
    let rebuild_context = OperationContext::new("index", "agent", "workspace_index_rebuild");
    workspace
        .rebuild_index(&rebuild_context, temp.path())
        .await
        .expect("rebuild");

    let find_context = OperationContext::new("find", "agent", "fs_find");
    let find_request = FsFindRequest {
        path: temp.path().to_path_buf(),
        pattern: "alpha".to_owned(),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: false,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: Vec::new(),
        limit: 20,
        budget: Default::default(),
    };
    let (find_page, _) = workspace
        .find_v2(&find_context, &find_request, None, None)
        .await
        .expect("indexed find");
    assert!(find_page.data.index_used);
    assert_eq!(
        find_page.data.index_freshness,
        chatcmd_runtime::IndexFreshness::Fresh
    );
    assert_eq!(find_page.data.items.len(), 1);

    let search_context = OperationContext::new("search", "agent", "fs_search");
    let search_request = FsSearchRequest {
        path: temp.path().to_path_buf(),
        query: "needle".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: false,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 10,
        limit: 20,
        max_snippet_bytes: 1024,
        budget: Default::default(),
    };
    let (search_page, _) = workspace
        .search_v2(&search_context, &search_request, None, None, |_| {})
        .await
        .expect("indexed search");
    assert!(search_page.data.index_used);
    assert_eq!(search_page.data.matches.len(), 2);

    let stats = workspace
        .batch_stat(
            &OperationContext::new("stat", "agent", "fs_batch_stat"),
            &FsBatchStatRequest {
                paths: vec![alpha.clone(), beta.clone()],
                version_strength: Default::default(),
                max_items: 2,
                budget: Default::default(),
            },
        )
        .await
        .expect("indexed stat");
    assert!(stats.index_used);
    assert_eq!(
        stats.index_freshness,
        chatcmd_runtime::IndexFreshness::Fresh
    );

    workspace.mark_index_stale(&alpha);
    let (fallback_page, _) = workspace
        .find_v2(&find_context, &find_request, None, None)
        .await
        .expect("direct fallback find");
    assert!(!fallback_page.data.index_used);
}

#[tokio::test]
async fn batch_stat_detects_unannounced_metadata_drift_and_marks_index_stale() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("drift.txt");
    std::fs::write(&path, "one").expect("write initial");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");

    std::fs::write(&path, "changed-size").expect("external mutation");
    let stats = workspace
        .batch_stat(
            &OperationContext::new("stat", "agent", "fs_batch_stat"),
            &FsBatchStatRequest {
                paths: vec![path.clone()],
                version_strength: Default::default(),
                max_items: 1,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch stat");
    assert!(stats.index_used);
    assert_eq!(stats.stale_entries_detected, 1);
    assert_eq!(
        stats.index_freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
}

#[tokio::test]
async fn batch_stat_enforces_aggregate_content_hash_budget_and_cancellation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = temp.path().join("first.bin");
    let second = temp.path().join("second.bin");
    std::fs::write(&first, b"12345678").expect("write first");
    std::fs::write(&second, b"abcdefgh").expect("write second");
    let workspace = service(temp.path());

    let stats = workspace
        .batch_stat(
            &OperationContext::new("stat-budget", "agent", "fs_batch_stat"),
            &FsBatchStatRequest {
                paths: vec![first.clone(), second.clone()],
                version_strength: VersionStrength::Content,
                max_items: 2,
                budget: FsBatchStatBudget {
                    timeout_ms: 10_000,
                    max_metadata_calls: 2,
                    max_bytes_read: 10,
                },
            },
        )
        .await
        .expect("batch stat");
    assert!(stats.items[0].ok);
    assert!(!stats.items[1].ok);
    assert_eq!(
        stats.items[1].error.as_ref().expect("error").code,
        "hashBudgetExceeded"
    );

    let cancelled = OperationContext::new("stat-cancel", "agent", "fs_batch_stat");
    cancelled.cancellation.cancel();
    let stats = workspace
        .batch_stat(
            &cancelled,
            &FsBatchStatRequest {
                paths: vec![first, second],
                version_strength: VersionStrength::Metadata,
                max_items: 2,
                budget: FsBatchStatBudget::default(),
            },
        )
        .await
        .expect("cancelled batch stat");
    assert!(stats.items.iter().all(|item| !item.ok));
    assert!(stats.items.iter().all(|item| {
        item.error.as_ref().map(|error| error.code.as_str()) == Some("batch_cancelled")
    }));
}

#[tokio::test]
async fn batch_read_enforces_aggregate_read_budget_deadline_and_cancellation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = temp.path().join("first.txt");
    let second = temp.path().join("second.txt");
    std::fs::write(&first, "a".repeat(128)).expect("write first");
    std::fs::write(&second, "b".repeat(128)).expect("write second");
    let workspace = service(temp.path());

    let read = |path| TextReadRequestV2 {
        path,
        range: TextReadRange::Byte {
            start: 0,
            limit: 128,
        },
        max_bytes: 128,
        include_line_endings: true,
        expected_version: None,
        budget: TextReadBudget {
            timeout_ms: 10_000,
            max_bytes_read: 128,
        },
    };
    let reads = workspace
        .batch_read(
            &OperationContext::new("read-budget", "agent", "fs_batch_read"),
            &FsBatchReadRequest {
                requests: vec![read(first.clone()), read(second.clone())],
                max_items: 2,
                max_total_output_bytes: 1024,
                concurrency: 2,
                budget: TextReadBudget {
                    timeout_ms: 10_000,
                    max_bytes_read: 20,
                },
            },
        )
        .await
        .expect("batch read");
    let total_bytes_read = reads
        .items
        .iter()
        .filter_map(|item| item.result.as_ref())
        .map(|result| result.bytes_read)
        .sum::<u64>();
    assert!(total_bytes_read <= 20, "read {total_bytes_read} bytes");
    assert_eq!(
        reads
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>(),
        vec![first.clone(), second.clone()]
    );

    let timed_out = workspace
        .batch_read(
            &OperationContext::new("read-timeout", "agent", "fs_batch_read"),
            &FsBatchReadRequest {
                requests: vec![read(first.clone())],
                max_items: 1,
                max_total_output_bytes: 1024,
                concurrency: 1,
                budget: TextReadBudget {
                    timeout_ms: 0,
                    max_bytes_read: 128,
                },
            },
        )
        .await
        .expect("timed out batch read");
    assert_eq!(
        timed_out.items[0].error.as_ref().expect("timeout").code,
        "batch_timeout"
    );

    let cancelled = OperationContext::new("read-cancel", "agent", "fs_batch_read");
    cancelled.cancellation.cancel();
    let cancelled_reads = workspace
        .batch_read(
            &cancelled,
            &FsBatchReadRequest {
                requests: vec![read(first), read(second)],
                max_items: 2,
                max_total_output_bytes: 1024,
                concurrency: 1,
                budget: TextReadBudget {
                    timeout_ms: 10_000,
                    max_bytes_read: 256,
                },
            },
        )
        .await
        .expect("cancelled batch read");
    assert!(cancelled_reads.items.iter().all(|item| !item.ok));
    assert!(cancelled_reads.items.iter().all(|item| {
        item.error.as_ref().map(|error| error.code.as_str()) == Some("batch_cancelled")
    }));
}

#[tokio::test]
async fn indexed_find_and_search_match_direct_fallback_on_stable_tree() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(temp.path().join("src")).expect("mkdir");
    std::fs::write(temp.path().join("src/a.rs"), "needle alpha\n").expect("write a");
    std::fs::write(temp.path().join("src/b.rs"), "needle beta\n").expect("write b");
    std::fs::write(temp.path().join("note.txt"), "needle note\n").expect("write note");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-parity", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");

    let find_request = FsFindRequest {
        path: temp.path().to_path_buf(),
        pattern: ".rs".to_owned(),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: true,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: vec!["rs".to_owned()],
        limit: 100,
        budget: Default::default(),
    };
    let context = OperationContext::new("find-parity", "agent", "fs_find");
    let (indexed, _) = workspace
        .find_v2(&context, &find_request, None, None)
        .await
        .expect("indexed find");
    assert!(indexed.data.index_used);
    let mut indexed_paths = indexed
        .data
        .items
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    indexed_paths.sort();

    workspace.mark_index_stale(temp.path());
    let (direct, _) = workspace
        .find_v2(&context, &find_request, None, None)
        .await
        .expect("direct find");
    assert!(!direct.data.index_used);
    let mut direct_paths = direct
        .data
        .items
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    direct_paths.sort();
    assert_eq!(indexed_paths, direct_paths);

    workspace
        .rebuild_index(
            &OperationContext::new("index-parity-2", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild again");
    let search_request = FsSearchRequest {
        path: temp.path().to_path_buf(),
        query: "needle".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: false,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 10,
        limit: 100,
        max_snippet_bytes: 1024,
        budget: Default::default(),
    };
    let search_context = OperationContext::new("search-parity", "agent", "fs_search");
    let (indexed_search, _) = workspace
        .search_v2(&search_context, &search_request, None, None, |_| {})
        .await
        .expect("indexed search");
    assert!(indexed_search.data.index_used);
    let mut indexed_matches = indexed_search
        .data
        .matches
        .iter()
        .map(|item| (item.path.clone(), item.line, item.column))
        .collect::<Vec<_>>();
    indexed_matches.sort();

    workspace.mark_index_stale(temp.path());
    let (direct_search, _) = workspace
        .search_v2(&search_context, &search_request, None, None, |_| {})
        .await
        .expect("direct search");
    assert!(!direct_search.data.index_used);
    let mut direct_matches = direct_search
        .data
        .matches
        .iter()
        .map(|item| (item.path.clone(), item.line, item.column))
        .collect::<Vec<_>>();
    direct_matches.sort();
    assert_eq!(indexed_matches, direct_matches);
}

#[tokio::test]
async fn restored_snapshot_is_stale_and_schema_mismatch_does_not_publish_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("one.txt"), "one").expect("write");
    let workspace = service(temp.path());
    let snapshot = RepositoryIndexSnapshot {
        root: temp.path().to_path_buf(),
        generation: 9,
        freshness: chatcmd_runtime::IndexFreshness::Fresh,
        indexed_bytes: 3,
        schema_version: 1,
        entries: vec![RepositoryIndexEntrySnapshot {
            relative_path_bytes: b"one.txt".to_vec(),
            display_path: "one.txt".to_owned(),
            entry_type: "file".to_owned(),
            size_bytes: 3,
            modified_at_ns: 0,
        }],
    };
    workspace
        .restore_index_snapshot(snapshot)
        .expect("restore snapshot");
    let status = workspace.index_status(temp.path()).expect("status");
    assert_eq!(status.generation, 9);
    assert_eq!(status.freshness, chatcmd_runtime::IndexFreshness::Stale);

    let other = service(temp.path());
    let error = other
        .restore_index_snapshot(RepositoryIndexSnapshot {
            root: temp.path().to_path_buf(),
            generation: 1,
            freshness: chatcmd_runtime::IndexFreshness::Fresh,
            indexed_bytes: 0,
            schema_version: u32::MAX,
            entries: Vec::new(),
        })
        .expect_err("schema mismatch");
    assert_eq!(error.code, "index_schema_mismatch");
    assert!(!other.index_status(temp.path()).expect("status").available);
}

#[tokio::test]
async fn concurrent_rebuilds_are_serialized_and_publish_monotonic_generations() {
    let temp = tempfile::tempdir().expect("tempdir");
    for index in 0..100 {
        std::fs::write(temp.path().join(format!("file-{index:03}.txt")), "data")
            .expect("write fixture");
    }
    let workspace = service(temp.path());
    let left = workspace.clone();
    let right = workspace.clone();
    let root_left = temp.path().to_path_buf();
    let root_right = root_left.clone();
    let (first, second) = tokio::join!(
        async move {
            left.rebuild_index(
                &OperationContext::new("rebuild-left", "agent", "workspace_index_rebuild"),
                &root_left,
            )
            .await
        },
        async move {
            right
                .rebuild_index(
                    &OperationContext::new("rebuild-right", "agent", "workspace_index_rebuild"),
                    &root_right,
                )
                .await
        }
    );
    first.expect("first rebuild");
    second.expect("second rebuild");
    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .generation,
        2
    );
}

#[tokio::test]
async fn ignored_content_is_not_indexed_or_returned_by_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join(".gitignore"), "secret/\n").expect("gitignore");
    std::fs::create_dir(temp.path().join("secret")).expect("secret dir");
    std::fs::write(temp.path().join("secret/token.txt"), "top-secret-value").expect("secret file");
    std::fs::write(temp.path().join("public.txt"), "public-value").expect("public file");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-ignore", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");
    let snapshot = workspace
        .export_index_snapshot(temp.path())
        .expect("export")
        .expect("snapshot");
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| !entry.display_path.starts_with("secret/"))
    );
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| !entry.display_path.contains("top-secret-value"))
    );
}

// macOS rejects byte-invalid filenames at the filesystem API boundary; Linux/other Unix
// filesystems that permit arbitrary filename bytes exercise the raw-byte snapshot path.
#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
async fn repository_snapshot_preserves_non_utf8_relative_path_bytes() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt as _};

    let temp = tempfile::tempdir().expect("tempdir");
    let raw_name = b"raw-\xff.txt".to_vec();
    let path = temp.path().join(OsString::from_vec(raw_name.clone()));
    std::fs::write(path, "data").expect("write non utf8");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-nonutf8", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");
    let snapshot = workspace
        .export_index_snapshot(temp.path())
        .expect("export")
        .expect("snapshot");
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.relative_path_bytes == raw_name)
    );
}

#[tokio::test]
async fn deleted_indexed_candidate_is_skipped_and_forces_direct_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("gone.txt");
    std::fs::write(&path, "gone").expect("write");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-delete", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");
    std::fs::remove_file(&path).expect("external delete");
    let request = FsFindRequest {
        path: temp.path().to_path_buf(),
        pattern: "gone".to_owned(),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: true,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: Vec::new(),
        limit: 100,
        budget: Default::default(),
    };
    let (page, _) = workspace
        .find_v2(
            &OperationContext::new("find-delete", "agent", "fs_find"),
            &request,
            None,
            None,
        )
        .await
        .expect("fallback find");
    assert!(!page.data.index_used);
    assert!(page.data.items.is_empty());
    assert!(page.data.stale_entries_detected >= 1);
    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
}

#[tokio::test]
async fn directory_rename_marks_stale_with_bounded_exact_tombstone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let old_dir = temp.path().join("old-dir");
    let new_dir = temp.path().join("new-dir");
    std::fs::create_dir(&old_dir).expect("create old dir");
    for index in 0..128 {
        std::fs::write(old_dir.join(format!("file-{index:03}.txt")), "data")
            .expect("write fixture");
    }
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-dir-rename", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");

    std::fs::rename(&old_dir, &new_dir).expect("rename directory");
    workspace.mark_index_stale(&old_dir);

    assert_eq!(
        workspace
            .index_status(temp.path())
            .expect("status")
            .freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
    let snapshot = workspace
        .export_index_snapshot(temp.path())
        .expect("export")
        .expect("snapshot");
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| entry.display_path != "old-dir")
    );
}

#[tokio::test]
async fn same_size_external_modify_is_detected_when_mtime_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("same.txt");
    std::fs::write(&path, "aaaa").expect("write initial");
    let workspace = service(temp.path());
    workspace
        .rebuild_index(
            &OperationContext::new("index-same-size", "agent", "workspace_index_rebuild"),
            temp.path(),
        )
        .await
        .expect("rebuild");
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&path, "bbbb").expect("same size modify");
    let stats = workspace
        .batch_stat(
            &OperationContext::new("stat-same-size", "agent", "fs_batch_stat"),
            &FsBatchStatRequest {
                paths: vec![path],
                version_strength: VersionStrength::Metadata,
                max_items: 1,
                budget: FsBatchStatBudget::default(),
            },
        )
        .await
        .expect("batch stat");
    assert_eq!(stats.stale_entries_detected, 1);
    assert_eq!(
        stats.index_freshness,
        chatcmd_runtime::IndexFreshness::Stale
    );
}

#[tokio::test]
async fn mixed_root_batch_stat_does_not_publish_ambiguous_generation() {
    let left = tempfile::tempdir().expect("left");
    let right = tempfile::tempdir().expect("right");
    let left_file = left.path().join("left.txt");
    let right_file = right.path().join("right.txt");
    std::fs::write(&left_file, "left").expect("write left");
    std::fs::write(&right_file, "right").expect("write right");
    let workspace = WorkspaceService::new(
        &[left.path().to_path_buf(), right.path().to_path_buf()],
        PolicyEngine::new(None, Arc::new(Reject)),
    )
    .expect("workspace");
    workspace
        .rebuild_index(
            &OperationContext::new("index-left", "agent", "workspace_index_rebuild"),
            left.path(),
        )
        .await
        .expect("left rebuild");
    workspace
        .rebuild_index(
            &OperationContext::new("index-right", "agent", "workspace_index_rebuild"),
            right.path(),
        )
        .await
        .expect("right rebuild");
    let result = workspace
        .batch_stat(
            &OperationContext::new("stat-mixed", "agent", "fs_batch_stat"),
            &FsBatchStatRequest {
                paths: vec![left_file, right_file],
                version_strength: VersionStrength::Metadata,
                max_items: 2,
                budget: FsBatchStatBudget::default(),
            },
        )
        .await
        .expect("mixed stat");
    assert!(!result.index_used);
    assert_eq!(result.index_generation, None);
    assert_eq!(
        result.index_freshness,
        chatcmd_runtime::IndexFreshness::Unknown
    );
    assert!(result.items.iter().all(|item| item.ok));
}

#[tokio::test]
async fn indexed_find_cursor_becomes_stale_after_generation_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    for index in 0..4 {
        std::fs::write(temp.path().join(format!("item-{index}.txt")), "data")
            .expect("write fixture");
    }
    let workspace = service(temp.path());
    let rebuild = OperationContext::new("index-find-cursor", "agent", "workspace_index_rebuild");
    workspace
        .rebuild_index(&rebuild, temp.path())
        .await
        .expect("initial rebuild");
    let request = FsFindRequest {
        path: temp.path().to_path_buf(),
        pattern: "item-".to_owned(),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: true,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: Vec::new(),
        limit: 1,
        budget: Default::default(),
    };
    let context = OperationContext::new("find-cursor", "agent", "fs_find");
    let (first, state) = workspace
        .find_v2(&context, &request, None, None)
        .await
        .expect("first page");
    assert!(first.data.index_used);
    assert!(first.has_more);
    let state = state.expect("cursor state");

    workspace
        .rebuild_index(&rebuild, temp.path())
        .await
        .expect("second rebuild");
    let error = workspace
        .find_v2(&context, &request, Some(&state), Some(&first.root_version))
        .await
        .expect_err("stale indexed cursor");
    assert_eq!(error.code, "cursor_stale");
}

#[tokio::test]
async fn indexed_search_cursor_becomes_stale_after_generation_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    for index in 0..4 {
        std::fs::write(
            temp.path().join(format!("item-{index}.txt")),
            format!("needle {index}\n"),
        )
        .expect("write fixture");
    }
    let workspace = service(temp.path());
    let rebuild = OperationContext::new("index-search-cursor", "agent", "workspace_index_rebuild");
    workspace
        .rebuild_index(&rebuild, temp.path())
        .await
        .expect("initial rebuild");
    let request = FsSearchRequest {
        path: temp.path().to_path_buf(),
        query: "needle".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: false,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 10,
        limit: 1,
        max_snippet_bytes: 1024,
        budget: Default::default(),
    };
    let context = OperationContext::new("search-cursor", "agent", "fs_search");
    let (first, state) = workspace
        .search_v2(&context, &request, None, None, |_| {})
        .await
        .expect("first page");
    assert!(first.data.index_used);
    assert!(first.has_more);
    let state = state.expect("cursor state");

    workspace
        .rebuild_index(&rebuild, temp.path())
        .await
        .expect("second rebuild");
    let error = workspace
        .search_v2(
            &context,
            &request,
            Some(&state),
            Some(&first.root_version),
            |_| {},
        )
        .await
        .expect_err("stale indexed cursor");
    assert_eq!(error.code, "cursor_stale");
}
