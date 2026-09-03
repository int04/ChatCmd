use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, FsSearchBudget, FsSearchRequest,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, SearchMode, TruncationReason,
    WorkspaceService,
};
use std::{collections::BTreeMap, fs, io::Write, path::Path, sync::Arc};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn policy() -> PolicyEngine {
    PolicyEngine::new(
        Some(ExecutionPolicy {
            default: PolicyDecision::Allow,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }),
        Arc::new(Approve),
    )
}

fn workspace(root: &Path) -> WorkspaceService {
    WorkspaceService::new(&[root.to_path_buf()], policy()).expect("workspace")
}

fn context(id: &str) -> OperationContext {
    OperationContext::new(id, "agent", "fs_search")
}

fn request(root: &Path, query: &str) -> FsSearchRequest {
    FsSearchRequest {
        path: root.to_path_buf(),
        query: query.to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: false,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: true,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 100,
        limit: 200,
        max_snippet_bytes: 8 * 1024,
        budget: FsSearchBudget::default(),
    }
}

async fn run(
    workspace: &WorkspaceService,
    context: &OperationContext,
    request: &FsSearchRequest,
) -> chatcmd_runtime::FsSearchScanPage {
    workspace
        .search_v2(context, request, None, None, |_| {})
        .await
        .expect("search")
        .0
}

#[tokio::test]
async fn literal_unicode_case_and_multiple_matches_work() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("sample.txt"),
        "Hello hello\nViệt NAM việt nam\nfoo foo foo\n",
    )
    .expect("write");
    let workspace = workspace(directory.path());

    let page = run(
        &workspace,
        &context("literal-ci"),
        &request(directory.path(), "hello"),
    )
    .await;
    assert_eq!(page.data.matches.len(), 2);
    assert_eq!(page.data.matches[0].line, 1);
    assert_eq!(page.data.matches[0].column, 1);
    assert_eq!(page.data.matches[1].column, 7);

    let mut unicode = request(directory.path(), "việt nam");
    unicode.case_sensitive = false;
    let page = run(&workspace, &context("unicode-ci"), &unicode).await;
    assert_eq!(page.data.matches.len(), 2);

    let mut exact = request(directory.path(), "Hello");
    exact.case_sensitive = true;
    let page = run(&workspace, &context("literal-cs"), &exact).await;
    assert_eq!(page.data.matches.len(), 1);

    let page = run(
        &workspace,
        &context("multiple"),
        &request(directory.path(), "foo"),
    )
    .await;
    assert_eq!(page.data.matches.len(), 3);
    assert_eq!(
        page.data
            .matches
            .iter()
            .map(|m| m.column)
            .collect::<Vec<_>>(),
        vec![1, 5, 9]
    );
}

#[tokio::test]
async fn regex_word_boundary_and_invalid_regex_are_typed() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("sample.txt"),
        "cat scatter cat42 cat\nabc-123\n",
    )
    .expect("write");
    let workspace = workspace(directory.path());

    let mut regex = request(directory.path(), r"abc-\d+");
    regex.mode = SearchMode::Regex;
    regex.case_sensitive = true;
    let page = run(&workspace, &context("regex"), &regex).await;
    assert_eq!(page.data.matches.len(), 1);
    assert_eq!(page.data.matches[0].match_text, "abc-123");

    let mut word = request(directory.path(), "cat");
    word.word_boundary = true;
    word.case_sensitive = true;
    let page = run(&workspace, &context("word"), &word).await;
    assert_eq!(page.data.matches.len(), 2);
    assert_eq!(
        page.data
            .matches
            .iter()
            .map(|m| m.column)
            .collect::<Vec<_>>(),
        vec![1, 19]
    );

    let mut invalid = request(directory.path(), "(");
    invalid.mode = SearchMode::Regex;
    let error = workspace
        .search_v2(&context("invalid"), &invalid, None, None, |_| {})
        .await
        .expect_err("invalid regex must fail");
    assert_eq!(error.code, "invalid_search_regex");
}

#[tokio::test]
async fn bom_crlf_offsets_context_and_snippet_bounds_are_correct() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("sample.txt");
    fs::write(
        &path,
        b"\xef\xbb\xbfprefix\r\nalpha needle omega\r\nsuffix\r\n",
    )
    .expect("write");
    let workspace = workspace(directory.path());
    let mut search = request(directory.path(), "needle");
    search.case_sensitive = true;
    search.context_before = 1;
    search.context_after = 1;
    search.max_snippet_bytes = 64;
    let page = run(&workspace, &context("offset"), &search).await;
    assert_eq!(page.data.matches.len(), 1);
    let found = &page.data.matches[0];
    assert_eq!(found.line, 2);
    assert_eq!(found.column, 7);
    assert_eq!(found.byte_offset, 3 + "prefix\r\n".len() as u64 + 6);
    assert_eq!(found.match_start, 6);
    assert_eq!(found.match_end, 12);
    assert_eq!(found.context_before, vec!["prefix"]);
    assert_eq!(found.context_after, vec!["suffix"]);
    assert!(!found.line_truncated);
}

#[tokio::test]
async fn item_and_output_budget_cursor_resume_without_duplicates_or_gaps() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("sample.txt"), "x x x x x x\n").expect("write");
    let workspace = workspace(directory.path());
    let cursor_context = context("cursor");
    let mut search = request(directory.path(), "x");
    search.case_sensitive = true;
    search.limit = 2;
    search.budget.max_output_bytes = 1024 * 1024;

    let (first, state1) = workspace
        .search_v2(&cursor_context, &search, None, None, |_| {})
        .await
        .expect("first page");
    assert_eq!(first.data.matches.len(), 2);
    assert_eq!(first.truncation_reason, Some(TruncationReason::ItemLimit));
    let state1 = state1.expect("cursor state");

    let (second, state2) = workspace
        .search_v2(
            &cursor_context,
            &search,
            Some(&state1),
            Some(&first.root_version),
            |_| {},
        )
        .await
        .expect("second page");
    assert_eq!(second.data.matches.len(), 2);
    let state2 = state2.expect("second cursor state");

    let (third, state3) = workspace
        .search_v2(
            &cursor_context,
            &search,
            Some(&state2),
            Some(&second.root_version),
            |_| {},
        )
        .await
        .expect("third page");
    assert_eq!(third.data.matches.len(), 2);
    assert!(state3.is_none());

    let offsets = first
        .data
        .matches
        .iter()
        .chain(second.data.matches.iter())
        .chain(third.data.matches.iter())
        .map(|m| m.byte_offset)
        .collect::<Vec<_>>();
    assert_eq!(offsets, vec![0, 2, 4, 6, 8, 10]);

    let mut tiny_output = request(directory.path(), "x");
    tiny_output.limit = 100;
    tiny_output.budget.max_output_bytes = 1;
    let (page, state) = workspace
        .search_v2(&context("output-budget"), &tiny_output, None, None, |_| {})
        .await
        .expect("bounded output");
    assert!(page.data.matches.is_empty());
    assert_eq!(page.truncation_reason, Some(TruncationReason::OutputLimit));
    assert!(state.is_some());
}

#[tokio::test]
async fn scan_budgets_and_cancellation_stop_with_typed_reasons() {
    let directory = tempfile::tempdir().expect("tempdir");
    for index in 0..4 {
        fs::write(directory.path().join(format!("{index}.txt")), "needle\n").expect("write");
    }
    let workspace = workspace(directory.path());

    let mut file_budget = request(directory.path(), "needle");
    file_budget.budget.max_files_scanned = 1;
    let page = run(&workspace, &context("file-budget"), &file_budget).await;
    assert_eq!(page.files_scanned, 1);
    assert_eq!(page.truncation_reason, Some(TruncationReason::FileBudget));

    let mut byte_budget = request(directory.path(), "needle");
    byte_budget.budget.max_bytes_scanned = 1;
    let page = run(&workspace, &context("byte-budget"), &byte_budget).await;
    assert_eq!(page.truncation_reason, Some(TruncationReason::ByteBudget));
    assert!(page.bytes_scanned >= 1);

    let cancelled = context("cancelled");
    cancelled.cancellation.cancel();
    let page = run(&workspace, &cancelled, &request(directory.path(), "needle")).await;
    assert_eq!(page.truncation_reason, Some(TruncationReason::Cancelled));
    assert_eq!(page.files_scanned, 0);
}

#[tokio::test]
async fn binary_invalid_utf8_include_exclude_and_file_size_are_reported() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("binary.bin"), b"needle\0binary").expect("write binary");
    fs::write(
        directory.path().join("invalid.txt"),
        b"before \xff needle\n",
    )
    .expect("write invalid");
    fs::write(directory.path().join("keep.rs"), "needle\n").expect("write keep");
    fs::write(directory.path().join("skip.txt"), "needle\n").expect("write skip");
    fs::write(directory.path().join("large.rs"), "needle needle needle\n").expect("write large");
    let workspace = workspace(directory.path());

    let mut search = request(directory.path(), "needle");
    search.include = vec!["*.rs".to_owned(), "*.txt".to_owned(), "*.bin".to_owned()];
    search.exclude = vec!["skip.txt".to_owned()];
    search.budget.max_file_bytes = 18;
    let page = run(&workspace, &context("skips"), &search).await;
    assert_eq!(page.data.binary_files_skipped, 1);
    assert!(page.data.files_skipped_by_size >= 1);
    assert!(page.data.errors_skipped >= 1);
    assert!(
        page.warnings
            .iter()
            .any(|warning| warning.code == "invalid_utf8_lossy")
    );
    assert!(
        page.data
            .matches
            .iter()
            .any(|m| m.path.ends_with("keep.rs"))
    );
    assert!(
        !page
            .data
            .matches
            .iter()
            .any(|m| m.path.ends_with("skip.txt"))
    );
}

#[tokio::test]
async fn timeout_mid_scan_cancellation_and_snippet_truncation_are_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("sample.txt"),
        format!("{} needle\nsecond needle\n", "x".repeat(256)),
    )
    .expect("write");
    let workspace = workspace(directory.path());

    let mut snippet = request(directory.path(), "needle");
    snippet.case_sensitive = true;
    snippet.max_snippet_bytes = 64;
    let page = run(&workspace, &context("snippet"), &snippet).await;
    assert!(page.data.matches[0].line_truncated);
    assert!(page.data.matches[0].line_text.len() <= 64);

    let mut timeout_request = request(directory.path(), "absent");
    timeout_request.budget.timeout_ms = 1;
    let (page, _) = workspace
        .search_v2(&context("timeout"), &timeout_request, None, None, |_| {
            std::thread::sleep(std::time::Duration::from_millis(5))
        })
        .await
        .expect("timeout search");
    assert_eq!(page.truncation_reason, Some(TruncationReason::TimeBudget));

    let cancellation_context = context("mid-cancel");
    let token = cancellation_context.cancellation.clone();
    let (page, _) = workspace
        .search_v2(
            &cancellation_context,
            &request(directory.path(), "needle"),
            None,
            None,
            move |_| token.cancel(),
        )
        .await
        .expect("mid-scan cancellation");
    assert_eq!(page.truncation_reason, Some(TruncationReason::Cancelled));
    assert!(page.files_scanned >= 1);
}

#[tokio::test]
async fn cursor_is_bound_to_owner_options_and_root_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("a.txt"), "x x x\n").expect("write");
    let workspace = workspace(directory.path());
    let owner = context("scope-owner");
    let mut search = request(directory.path(), "x");
    search.limit = 1;
    let (first, state) = workspace
        .search_v2(&owner, &search, None, None, |_| {})
        .await
        .expect("first");
    let state = state.expect("state");

    let other_owner = OperationContext::new("scope-other", "other-agent", "fs_search");
    let error = workspace
        .search_v2(
            &other_owner,
            &search,
            Some(&state),
            Some(&first.root_version),
            |_| {},
        )
        .await
        .expect_err("owner mismatch");
    assert_eq!(error.code, "cursor_scope_mismatch");

    let mut changed_options = search.clone();
    changed_options.case_sensitive = true;
    let (first, state) = workspace
        .search_v2(&owner, &search, None, None, |_| {})
        .await
        .expect("fresh first");
    let state = state.expect("fresh state");
    let error = workspace
        .search_v2(
            &owner,
            &changed_options,
            Some(&state),
            Some(&first.root_version),
            |_| {},
        )
        .await
        .expect_err("option mismatch");
    assert_eq!(error.code, "cursor_scope_mismatch");

    let (first, state) = workspace
        .search_v2(&owner, &search, None, None, |_| {})
        .await
        .expect("stale first");
    let state = state.expect("stale state");
    let mut created = fs::File::create(directory.path().join("new-file.txt")).expect("new file");
    created.write_all(b"x\n").expect("write new file");
    created.sync_all().expect("sync new file");
    let error = workspace
        .search_v2(
            &owner,
            &search,
            Some(&state),
            Some(&first.root_version),
            |_| {},
        )
        .await
        .expect_err("root mutation must stale cursor");
    assert_eq!(error.code, "cursor_stale");
}
