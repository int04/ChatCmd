use chatcmd_runtime::{
    ApplyEditsBudget, ApplyEditsRequest, ApprovalDecision, BoxFuture, EditColumnEncoding,
    EditCoordinateSystem, ExecutionPolicy, FsStatBudget, FsStatRequest, NullEventSink,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeConfig, RuntimeResult,
    ShellCreateRequest, ShellRuntime, ShellWriteRequest, SystemLocalDevice, TextEdit, TextPosition,
    VersionStrength, WorkspaceService,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn metadata_version(workspace: &WorkspaceService, path: &std::path::Path) -> String {
    workspace
        .stat_v2(
            None,
            &FsStatRequest {
                path: path.to_path_buf(),
                version_strength: VersionStrength::Metadata,
                hash_algorithm: None,
                budget: FsStatBudget::default(),
            },
        )
        .await
        .expect("capture version")
        .version_token
}

#[tokio::test]
async fn apply_edits_handles_byte_ranges_dry_run_and_stale_versions() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace =
        WorkspaceService::new(&[directory.path().to_path_buf()], policy()).expect("workspace");
    let file = directory.path().join("edits.txt");
    std::fs::write(&file, "zero one two three").expect("write sample");
    let version = metadata_version(&workspace, &file).await;
    let request = ApplyEditsRequest {
        path: file.clone(),
        expected_version: version.clone(),
        coordinate_system: EditCoordinateSystem::Byte,
        column_encoding: None,
        edits: vec![
            TextEdit {
                start_byte: Some(13),
                end_byte: Some(18),
                start: None,
                end: None,
                text: "THREE".to_owned(),
            },
            TextEdit {
                start_byte: Some(0),
                end_byte: Some(4),
                start: None,
                end: None,
                text: "ZERO".to_owned(),
            },
            TextEdit {
                start_byte: Some(8),
                end_byte: Some(8),
                start: None,
                end: None,
                text: "small ".to_owned(),
            },
        ],
        dry_run: true,
        preserve_line_endings: true,
        preserve_bom: true,
        budget: ApplyEditsBudget::default(),
    };
    let dry_run = workspace
        .apply_edits(
            &OperationContext::new("dry", "agent", "fs_apply_edits"),
            &request,
        )
        .await
        .expect("dry run");
    assert!(!dry_run.applied);
    assert_eq!(
        std::fs::read_to_string(&file).expect("unchanged file"),
        "zero one two three"
    );

    let applied = workspace
        .apply_edits(
            &OperationContext::new("apply", "agent", "fs_apply_edits"),
            &ApplyEditsRequest {
                dry_run: false,
                ..request.clone()
            },
        )
        .await
        .expect("apply edits");
    assert!(applied.applied);
    assert_ne!(applied.old_version, applied.new_version);
    assert_eq!(
        std::fs::read_to_string(&file).expect("edited file"),
        "ZERO onesmall  two THREE"
    );
    let stale = workspace
        .apply_edits(
            &OperationContext::new("retry", "agent", "fs_apply_edits"),
            &ApplyEditsRequest {
                dry_run: false,
                ..request
            },
        )
        .await
        .expect_err("stale retry");
    assert_eq!(stale.code, "targetReplaced");
}

#[tokio::test]
async fn apply_edits_supports_unicode_line_columns_crlf_and_rejects_overlap() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace =
        WorkspaceService::new(&[directory.path().to_path_buf()], policy()).expect("workspace");
    let file = directory.path().join("unicode.txt");
    std::fs::write(
        &file,
        b"\xef\xbb\xbfalpha\r\na\xf0\x9f\x98\x80e\xcc\x81\r\nomega",
    )
    .expect("write UTF-8 sample");
    let version = metadata_version(&workspace, &file).await;
    let request = ApplyEditsRequest {
        path: file.clone(),
        expected_version: version,
        coordinate_system: EditCoordinateSystem::LineColumn,
        column_encoding: Some(EditColumnEncoding::Utf8CodePoint),
        edits: vec![TextEdit {
            start_byte: None,
            end_byte: None,
            start: Some(TextPosition { line: 2, column: 2 }),
            end: Some(TextPosition { line: 2, column: 3 }),
            text: "EMOJI\nNEXT".to_owned(),
        }],
        dry_run: false,
        preserve_line_endings: true,
        preserve_bom: true,
        budget: ApplyEditsBudget::default(),
    };
    workspace
        .apply_edits(
            &OperationContext::new("unicode", "agent", "fs_apply_edits"),
            &request,
        )
        .await
        .expect("unicode edit");
    assert_eq!(
        std::fs::read(&file).expect("edited bytes"),
        b"\xef\xbb\xbfalpha\r\naEMOJI\r\nNEXTe\xcc\x81\r\nomega"
    );

    let version = metadata_version(&workspace, &file).await;
    let overlap = workspace
        .apply_edits(
            &OperationContext::new("overlap", "agent", "fs_apply_edits"),
            &ApplyEditsRequest {
                path: file,
                expected_version: version,
                coordinate_system: EditCoordinateSystem::Byte,
                column_encoding: None,
                edits: vec![
                    TextEdit {
                        start_byte: Some(3),
                        end_byte: Some(8),
                        start: None,
                        end: None,
                        text: String::new(),
                    },
                    TextEdit {
                        start_byte: Some(7),
                        end_byte: Some(9),
                        start: None,
                        end: None,
                        text: String::new(),
                    },
                ],
                dry_run: true,
                preserve_line_endings: true,
                preserve_bom: true,
                budget: ApplyEditsBudget::default(),
            },
        )
        .await
        .expect_err("overlap rejected");
    assert_eq!(overlap.code, "overlappingEdits");
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

fn runtime(root: PathBuf, max_sessions: usize) -> ShellRuntime {
    ShellRuntime::new(
        RuntimeConfig {
            roots: vec![root],
            max_sessions,
            ..RuntimeConfig::default()
        },
        policy(),
        Arc::new(NullEventSink),
    )
}

fn create_request(root: PathBuf, request_id: &str) -> ShellCreateRequest {
    ShellCreateRequest {
        request_id: request_id.to_owned(),
        working_directory: Some(root),
        executable: None,
        arguments: Vec::new(),
        environment: BTreeMap::new(),
        columns: Some(100),
        rows: Some(24),
    }
}

#[test]
fn exactly_one_local_device() {
    let provider = SystemLocalDevice::new("test");
    assert_eq!(provider.list().len(), 1);
    assert_eq!(
        provider.get("local").expect("local device").device_id,
        "local"
    );
}

#[tokio::test]
async fn shell_lifecycle_timeout_duplicate_and_force_close() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 2);
    let context = OperationContext::new("create-1", "agent", "shell_create");
    let request = create_request(directory.path().to_path_buf(), "create-1");
    let created = runtime
        .create(&context, request.clone())
        .await
        .expect("create shell");
    let duplicate = runtime
        .create(&context, request)
        .await
        .expect("duplicate replay");
    assert_eq!(created.session_id, duplicate.session_id);

    let timed_out = runtime
        .wait(&created.session_id, Duration::from_millis(20))
        .await
        .expect("wait");
    assert!(timed_out.wait_timed_out);
    assert_eq!(
        runtime
            .inspect(&created.session_id)
            .await
            .expect("inspect")
            .status,
        "running"
    );

    let command = if cfg!(windows) {
        "Write-Output chatcmd-ready"
    } else {
        "printf 'chatcmd-ready\\n'"
    };
    runtime
        .write(
            &OperationContext::new("write-1", "agent", "shell_write"),
            ShellWriteRequest {
                request_id: "write-1".to_owned(),
                session_id: created.session_id.clone(),
                text: command.to_owned(),
                append_new_line: true,
            },
        )
        .await
        .expect("write shell");
    let output_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = runtime
            .read(&created.session_id, 0, 100)
            .await
            .expect("read shell");
        if output
            .events
            .iter()
            .any(|event| event.data.contains("chatcmd-ready"))
        {
            break;
        }
        assert!(
            Instant::now() < output_deadline,
            "PTY output did not arrive before the deadline: {output:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    runtime
        .close(
            &OperationContext::new("close-1", "agent", "shell_close"),
            &created.session_id,
            true,
        )
        .await
        .expect("force close");
    assert!(
        runtime
            .list()
            .await
            .expect("list after close")
            .iter()
            .all(|session| session.session_id != created.session_id)
    );
    runtime
        .read(&created.session_id, 0, 100)
        .await
        .expect("retired shell output remains readable");
}

#[tokio::test]
async fn shell_wait_retires_exited_session_and_keeps_replay() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 2);
    let created = runtime
        .create(
            &OperationContext::new("wait-retire-create", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "wait-retire-create"),
        )
        .await
        .expect("create shell");
    runtime
        .write(
            &OperationContext::new("wait-retire-exit", "agent", "shell_write"),
            ShellWriteRequest {
                request_id: "wait-retire-exit".to_owned(),
                session_id: created.session_id.clone(),
                text: "exit".to_owned(),
                append_new_line: true,
            },
        )
        .await
        .expect("exit shell");
    let exited = runtime
        .wait(&created.session_id, Duration::from_secs(5))
        .await
        .expect("wait for exit");
    assert!(exited.completed);
    assert!(runtime.list().await.expect("list").is_empty());
    runtime
        .read(&created.session_id, 0, 100)
        .await
        .expect("retired replay remains readable");
}

#[tokio::test]
async fn shell_reader_retires_exited_session_without_wait() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 2);
    let created = runtime
        .create(
            &OperationContext::new("reader-retire-create", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "reader-retire-create"),
        )
        .await
        .expect("create shell");
    runtime
        .write(
            &OperationContext::new("reader-retire-exit", "agent", "shell_write"),
            ShellWriteRequest {
                request_id: "reader-retire-exit".to_owned(),
                session_id: created.session_id.clone(),
                text: "exit".to_owned(),
                append_new_line: true,
            },
        )
        .await
        .expect("exit shell");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let active = runtime
            .list()
            .await
            .expect("list")
            .iter()
            .any(|session| session.session_id == created.session_id);
        if !active {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "reader did not retire exited shell"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    runtime
        .read(&created.session_id, 0, 100)
        .await
        .expect("reader-retired replay remains readable");
}

#[tokio::test]
async fn cancellation_and_session_backpressure_are_explicit() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 1);
    let cancelled = OperationContext::new("cancelled", "agent", "shell_create");
    cancelled.cancellation.cancel();
    let error = runtime
        .create(
            &cancelled,
            create_request(directory.path().to_path_buf(), "cancelled"),
        )
        .await
        .expect_err("cancelled create");
    assert_eq!(error.code, "cancelled");

    let first = runtime
        .create(
            &OperationContext::new("first", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "first"),
        )
        .await
        .expect("first shell");
    let busy = runtime
        .create(
            &OperationContext::new("second", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "second"),
        )
        .await
        .expect_err("session limit");
    assert_eq!(busy.code, "device_busy");
    runtime
        .close(
            &OperationContext::new("close", "agent", "shell_close"),
            &first.session_id,
            true,
        )
        .await
        .expect("close shell");
}

#[tokio::test]
async fn shell_absolute_external_working_directory_is_auto_allowed() {
    let workspace = tempfile::tempdir().expect("workspace directory");
    let external = tempfile::tempdir().expect("external directory");
    let runtime = runtime(workspace.path().to_path_buf(), 2);
    let expected = external
        .path()
        .canonicalize()
        .expect("canonical external directory");

    let created = runtime
        .create(
            &OperationContext::new("external-absolute", "agent", "shell_create"),
            create_request(external.path().to_path_buf(), "external-absolute"),
        )
        .await
        .expect("absolute external cwd must be auto-allowed");
    assert_eq!(created.initial_working_directory, expected);

    runtime
        .close(
            &OperationContext::new("external-close", "agent", "shell_close"),
            &created.session_id,
            true,
        )
        .await
        .expect("close external shell");
}

#[tokio::test]
async fn workspace_reads_line_ranges_and_replaces_text_exactly() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace =
        WorkspaceService::new(&[directory.path().to_path_buf()], policy()).expect("workspace");
    let file = directory.path().join("sample.txt");
    std::fs::write(&file, "one\ntwo\nthree\nfour\n").expect("write sample");

    let full = workspace
        .read_text(&file, 1_000)
        .await
        .expect("read full text");
    assert_eq!(full.content, "one\ntwo\nthree\nfour\n");
    assert!(!full.truncated);

    let range = workspace
        .read_text_range(&file, 1_000, 2, Some(2))
        .await
        .expect("read line range");
    assert_eq!(range.content, "two\nthree");
    assert_eq!(range.start_line, 2);
    assert_eq!(range.end_line, 3);
    assert_eq!(range.total_lines, 4);
    assert!(range.truncated);

    workspace
        .replace_text(
            &OperationContext::new("replace", "agent", "fs_replace_text"),
            &file,
            "two",
            "TWO",
            1,
        )
        .await
        .expect("replace exact text");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read replaced file"),
        "one\nTWO\nthree\nfour\n"
    );

    let windows_file = directory.path().join("windows.txt");
    std::fs::write(&windows_file, "one\r\ntwo\r\nthree\r\n").expect("write CRLF sample");
    let windows_range = workspace
        .read_text_range(&windows_file, 1_000, 2, Some(2))
        .await
        .expect("read normalized CRLF range");
    assert_eq!(windows_range.content, "two\nthree");
    workspace
        .replace_text(
            &OperationContext::new("replace-crlf", "agent", "fs_replace_text"),
            &windows_file,
            &windows_range.content,
            "TWO\nTHREE",
            1,
        )
        .await
        .expect("replace normalized text in CRLF file");
    assert_eq!(
        std::fs::read_to_string(&windows_file).expect("read CRLF replacement"),
        "one\r\nTWO\r\nTHREE\r\n"
    );

    let ambiguous = workspace
        .replace_text(
            &OperationContext::new("replace-ambiguous", "agent", "fs_replace_text"),
            &file,
            "e",
            "E",
            1,
        )
        .await
        .expect_err("mismatched occurrence count must fail");
    assert_eq!(ambiguous.code, "text_match_count_mismatch");
}

#[tokio::test]
async fn workspace_search_respects_default_gitignore_exclude_and_direct_root_override() {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path();
    std::fs::create_dir_all(root.join("src")).expect("create src");
    std::fs::create_dir_all(root.join("node_modules")).expect("create node_modules");
    std::fs::create_dir_all(root.join("ignored")).expect("create ignored");
    std::fs::create_dir_all(root.join("keep")).expect("create keep");
    std::fs::write(root.join(".gitignore"), "ignored/\n").expect("write gitignore");
    std::fs::write(root.join("src/source.txt"), "needle source\n").expect("write source");
    std::fs::write(
        root.join("node_modules/dependency.txt"),
        "needle dependency\n",
    )
    .expect("write dependency");
    std::fs::write(root.join("ignored/generated.txt"), "needle ignored\n").expect("write ignored");
    std::fs::write(root.join("keep/excluded.txt"), "needle excluded\n").expect("write exclude");

    let workspace = WorkspaceService::new(&[root.to_path_buf()], policy()).expect("workspace");
    let normal = workspace
        .search(
            root,
            "needle",
            false,
            100,
            1_048_576,
            false,
            vec!["keep/".to_owned()],
            |_| {},
        )
        .await
        .expect("normal search");
    assert_eq!(normal.len(), 1);
    assert!(
        normal[0]["path"]
            .as_str()
            .is_some_and(|path| path.contains("source.txt"))
    );

    let include_ignored = workspace
        .search(
            root,
            "needle",
            false,
            100,
            1_048_576,
            true,
            vec!["keep/".to_owned()],
            |_| {},
        )
        .await
        .expect("include ignored search");
    let include_paths = include_ignored
        .iter()
        .filter_map(|value| value["path"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(include_paths.len(), 3);
    assert!(include_paths.iter().any(|path| path.contains("source.txt")));
    assert!(
        include_paths
            .iter()
            .any(|path| path.contains("dependency.txt"))
    );
    assert!(
        include_paths
            .iter()
            .any(|path| path.contains("generated.txt"))
    );
    assert!(
        !include_paths
            .iter()
            .any(|path| path.contains("excluded.txt"))
    );

    let direct_ignored_root = workspace
        .search(
            &root.join("node_modules"),
            "needle",
            false,
            100,
            1_048_576,
            false,
            Vec::new(),
            |_| {},
        )
        .await
        .expect("direct ignored root search");
    assert_eq!(direct_ignored_root.len(), 1);
    assert!(
        direct_ignored_root[0]["path"]
            .as_str()
            .is_some_and(|path| path.contains("dependency.txt"))
    );
}

#[tokio::test]
async fn workspace_traversal_is_denied() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace =
        WorkspaceService::new(&[directory.path().to_path_buf()], policy()).expect("workspace");
    let outside = directory
        .path()
        .parent()
        .expect("parent")
        .join("outside.txt");
    let error = workspace
        .read_text(&outside, 100)
        .await
        .expect_err("outside path denied");
    assert!(matches!(
        error.code.as_str(),
        "not_found" | "path_outside_allowed_scope"
    ));
}
