use super::*;
use crate::{ApprovalDecision, BoxFuture, FsFindBudget, PolicyContext, PolicyEngine};
use std::{collections::HashSet, fs::File, sync::Arc};
use tempfile::tempdir;

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

fn request(path: &Path, pattern: &str) -> FsFindRequest {
    FsFindRequest {
        path: path.to_path_buf(),
        pattern: pattern.to_owned(),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: false,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: Vec::new(),
        limit: 200,
        budget: FsFindBudget::default(),
    }
}

#[test]
fn patterns_compile_and_invalid_patterns_fail_before_scan() {
    let temp = tempdir().expect("tempdir");
    let literal = compile_pattern(&request(temp.path(), "FileSystem")).expect("literal");
    assert!(matches!(literal, CompiledPattern::Literal(ref value) if value == "filesystem"));

    let mut glob = request(temp.path(), "**/*.rs");
    glob.pattern_mode = FindPatternMode::Glob;
    assert!(matches!(
        compile_pattern(&glob).expect("glob"),
        CompiledPattern::Glob(_)
    ));

    glob.pattern = "[".to_owned();
    assert_eq!(
        compile_pattern(&glob).err().expect("invalid pattern").code,
        "invalid_find_pattern"
    );
    glob.pattern_mode = FindPatternMode::Regex;
    glob.pattern = "(".to_owned();
    assert_eq!(
        compile_pattern(&glob).err().expect("invalid pattern").code,
        "invalid_find_pattern"
    );
}

#[tokio::test]
async fn ignore_filters_depth_extensions_and_pattern_modes_work() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("src/deep")).expect("src");
    std::fs::create_dir_all(temp.path().join("target")).expect("target");
    std::fs::create_dir_all(temp.path().join("ignored")).expect("ignored");
    std::fs::write(temp.path().join(".gitignore"), "ignored/\n").expect("gitignore");
    File::create(temp.path().join("src/main.rs")).expect("main");
    File::create(temp.path().join("src/deep/lib.rs")).expect("lib");
    File::create(temp.path().join("target/generated.rs")).expect("target file");
    File::create(temp.path().join("ignored/skip.rs")).expect("ignored file");

    let workspace = service(temp.path());
    let context = OperationContext::new("r", "a", "fs_find");
    let mut glob = request(temp.path(), "**/*.rs");
    glob.pattern_mode = FindPatternMode::Glob;
    glob.extensions = vec!["rs".to_owned()];
    let (page, _) = workspace
        .find_v2(&context, &glob, None, None)
        .await
        .expect("glob page");
    let paths: Vec<_> = page
        .data
        .items
        .iter()
        .map(|item| item.path.replace('\\', "/"))
        .collect();
    assert!(paths.iter().any(|path| path.ends_with("src/main.rs")));
    assert!(paths.iter().any(|path| path.ends_with("src/deep/lib.rs")));
    assert!(!paths.iter().any(|path| path.contains("/target/")));
    assert!(!paths.iter().any(|path| path.contains("/ignored/")));

    let mut regex = request(temp.path(), r"^src/.+\.rs$");
    regex.pattern_mode = FindPatternMode::Regex;
    regex.max_depth = 2;
    let (page, _) = workspace
        .find_v2(&context, &regex, None, None)
        .await
        .expect("regex page");
    assert_eq!(page.data.items.len(), 1);
    assert!(page.data.items[0].path.ends_with("main.rs"));
}

#[tokio::test]
async fn include_ignored_hidden_exclude_and_entry_type_filters_work() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("target")).expect("target");
    std::fs::create_dir_all(temp.path().join("keep-dir")).expect("keep dir");
    File::create(temp.path().join("target/generated.rs")).expect("generated");
    File::create(temp.path().join(".hidden.rs")).expect("hidden");
    File::create(temp.path().join("keep-dir/value.rs")).expect("value");

    let workspace = service(temp.path());
    let context = OperationContext::new("filters", "a", "fs_find");

    let mut included = request(temp.path(), "**/*.rs");
    included.pattern_mode = FindPatternMode::Glob;
    included.include_ignored = true;
    included.include_hidden = true;
    included.exclude = vec!["keep-dir/".to_owned()];
    included.entry_types = vec![FindEntryType::File];
    let (page, _) = workspace
        .find_v2(&context, &included, None, None)
        .await
        .expect("included page");
    let paths: Vec<_> = page
        .data
        .items
        .iter()
        .map(|item| item.path.replace('\\', "/"))
        .collect();
    assert!(
        paths
            .iter()
            .any(|path| path.ends_with("target/generated.rs"))
    );
    assert!(paths.iter().any(|path| path.ends_with(".hidden.rs")));
    assert!(!paths.iter().any(|path| path.contains("/keep-dir/")));
    assert!(page.data.items.iter().all(|item| item.entry_type == "file"));
}

#[tokio::test]
async fn symlink_targets_are_not_traversed() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("root");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    File::create(outside.join("secret.txt")).expect("secret");

    #[cfg(unix)]
    let link_result = std::os::unix::fs::symlink(&outside, root.join("link"));
    #[cfg(windows)]
    let link_result = std::os::windows::fs::symlink_dir(&outside, root.join("link"));
    if link_result.is_err() {
        return;
    }

    let workspace = service(&root);
    let context = OperationContext::new("symlink", "a", "fs_find");
    let req = request(&root, "secret");
    let (page, _) = workspace
        .find_v2(&context, &req, None, None)
        .await
        .expect("symlink page");
    assert!(page.data.items.is_empty());
}

#[tokio::test]
async fn budget_cancellation_and_cursor_continuation_are_bounded() {
    let temp = tempdir().expect("tempdir");
    for index in 0..20 {
        File::create(temp.path().join(format!("file-{index:02}.txt"))).expect("create");
    }
    let workspace = service(temp.path());
    let context = OperationContext::new("r", "a", "fs_find");
    let mut req = request(temp.path(), "file-");
    req.limit = 3;
    let (first, state) = workspace
        .find_v2(&context, &req, None, None)
        .await
        .expect("first");
    assert_eq!(first.data.items.len(), 3);
    assert!(first.has_more);
    assert!(first.entries_scanned <= 4);
    let state = state.expect("state");
    let (second, _) = workspace
        .find_v2(&context, &req, Some(&state), Some(&first.root_version))
        .await
        .expect("second");
    let first_paths: HashSet<_> = first
        .data
        .items
        .iter()
        .map(|item| item.path.clone())
        .collect();
    assert!(
        second
            .data
            .items
            .iter()
            .all(|item| !first_paths.contains(&item.path))
    );

    let mut bounded = request(temp.path(), "never-match");
    bounded.budget.max_entries_scanned = 2;
    let (page, _) = workspace
        .find_v2(&context, &bounded, None, None)
        .await
        .expect("bounded");
    assert_eq!(page.truncation_reason, Some(TruncationReason::ItemLimit));
    assert_eq!(page.entries_scanned, 2);

    let mut timed_out = request(temp.path(), "file-");
    timed_out.budget.timeout_ms = 0;
    let (page, _) = workspace
        .find_v2(&context, &timed_out, None, None)
        .await
        .expect("timed out");
    assert_eq!(page.truncation_reason, Some(TruncationReason::TimeBudget));
    assert_eq!(page.entries_scanned, 0);

    let cancelled = OperationContext::new("r2", "a", "fs_find");
    cancelled.cancellation.cancel();
    let (page, _) = workspace
        .find_v2(&cancelled, &req, None, None)
        .await
        .expect("cancelled");
    assert_eq!(page.truncation_reason, Some(TruncationReason::Cancelled));
    assert_eq!(page.entries_scanned, 0);
}

#[tokio::test]
async fn cursor_is_bound_to_owner_options_and_server_state() {
    let temp = tempdir().expect("tempdir");
    for index in 0..5 {
        File::create(temp.path().join(format!("file-{index}.txt"))).expect("create");
    }
    let workspace = service(temp.path());
    let context = OperationContext::new("r", "owner-a", "fs_find");
    let mut req = request(temp.path(), "file-");
    req.limit = 1;
    let (page, state) = workspace
        .find_v2(&context, &req, None, None)
        .await
        .expect("page");
    let state = state.expect("state");
    let other = OperationContext::new("r", "owner-b", "fs_find");
    let error = workspace
        .find_v2(&other, &req, Some(&state), Some(&page.root_version))
        .await
        .expect_err("scope mismatch");
    assert_eq!(error.code, "cursor_scope_mismatch");

    let missing = workspace
        .find_v2(
            &context,
            &req,
            Some("missing-state"),
            Some(&page.root_version),
        )
        .await
        .expect_err("expired");
    assert_eq!(missing.code, "cursor_expired");
}

#[tokio::test]
#[ignore = "Plan 05 generated 100k-entry traversal benchmark"]
async fn fs_find_large_tree_stops_after_first_page() {
    let temp = tempdir().expect("tempdir");
    for index in 0..100_001_u32 {
        File::create(temp.path().join(format!("entry-{index:06}.txt"))).expect("create");
    }
    let workspace = service(temp.path());
    let context = OperationContext::new("benchmark", "agent", "fs_find");
    let mut req = request(temp.path(), "entry-");
    req.limit = 10;
    let started = Instant::now();
    let (page, _) = workspace
        .find_v2(&context, &req, None, None)
        .await
        .expect("page");
    eprintln!(
        "entries_scanned={} elapsed_ms={}",
        page.entries_scanned,
        started.elapsed().as_millis()
    );
    assert_eq!(page.data.items.len(), 10);
    assert!(page.entries_scanned <= 11);
}
