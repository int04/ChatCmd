mod support;

use chatcmd_runtime::{
    AtomicWriteOptions, FsSearchBudget, FsSearchRequest, FsStatBudget, FsStatRequest,
    GitRunOptions, GitService, OperationContext, SearchMode, TextReadBudget, TextReadRange,
    TextReadRequestV2, VersionStrength,
};
use std::{
    io::{Read as _, Write as _},
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
};
use support::{
    fault_injection::{FaultAction, FaultGate, FaultPoint},
    fixtures::{MARKER, write_large_file, write_sparse_file, write_tree},
    process_helper::{kill_at_marker, spawn_test_helper},
    resource_probe::ResourceProbe,
    workspace,
};
use tokio_util::sync::CancellationToken;

#[test]
fn named_fault_gate_controls_exact_interleaving_without_sleep() {
    let (reached_sender, reached_receiver) = std::sync::mpsc::sync_channel(0);
    let (release_sender, release_receiver) = std::sync::mpsc::sync_channel(0);
    let gate = FaultGate::new(
        FaultPoint::BeforeAtomicReplace,
        FaultAction::Error(std::io::ErrorKind::Other),
        reached_sender,
        release_receiver,
    );
    let worker = std::thread::spawn(move || gate.trigger(FaultPoint::BeforeAtomicReplace));
    assert_eq!(
        reached_receiver.recv().expect("fault point reached"),
        FaultPoint::BeforeAtomicReplace
    );
    release_sender.send(()).expect("release fault point");
    let error = worker
        .join()
        .expect("fault worker")
        .expect_err("fault must be injected");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
}

fn read_request(path: &Path, start: u64, limit: usize) -> TextReadRequestV2 {
    TextReadRequestV2 {
        path: path.to_path_buf(),
        range: TextReadRange::Byte { start, limit },
        max_bytes: limit,
        include_line_endings: true,
        expected_version: None,
        budget: TextReadBudget {
            timeout_ms: 30_000,
            max_bytes_read: 128 * 1024,
        },
    }
}

async fn metadata_version(workspace: &chatcmd_runtime::WorkspaceService, path: &Path) -> String {
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
        .expect("stat fixture")
        .version_token
}

#[tokio::test]
async fn ten_mibibyte_ranges_are_streamed_and_resource_bounded() {
    let directory = tempfile::tempdir().expect("temp directory");
    let fixture = write_large_file(
        &directory.path().join("large.txt"),
        10 * 1024 * 1024,
        0x5eed,
    );
    let workspace = workspace(directory.path());
    let probe = ResourceProbe::default();

    assert_eq!(fixture.sha256.len(), 64);
    for offset in fixture.marker_offsets {
        let result = workspace
            .read_text_v2(None, &read_request(&fixture.path, offset, MARKER.len()))
            .await
            .expect("read marker range");
        probe.add_entry();
        probe.add_bytes(result.bytes_read);
        probe.observe_buffered(result.content.len() as u64);
        assert_eq!(result.content.as_bytes(), MARKER);
        assert!(result.bytes_read <= (MARKER.len() + 11) as u64);
        assert_eq!(result.size_bytes, fixture.size);
    }

    let resources = probe.snapshot();
    assert_eq!(resources.entries, 3);
    assert!(resources.bytes <= 3 * (MARKER.len() + 11) as u64);
    assert!(resources.maximum_buffered <= MARKER.len() as u64);
}

#[tokio::test]
async fn sparse_gibibyte_ranges_do_not_scale_reads_with_file_size() {
    let directory = tempfile::tempdir().expect("temp directory");
    let fixture = write_sparse_file(&directory.path().join("sparse.bin"), 1024 * 1024 * 1024);
    let workspace = workspace(directory.path());

    for offset in fixture.marker_offsets {
        let result = workspace
            .read_text_v2(None, &read_request(&fixture.path, offset, MARKER.len()))
            .await
            .expect("read sparse marker");
        assert_eq!(result.content.as_bytes(), MARKER);
        assert!(result.bytes_read <= (MARKER.len() + 11) as u64);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_expected_version_writers_have_one_commit_winner() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("race.txt");
    std::fs::write(&path, "baseline").expect("seed race target");
    let workspace = Arc::new(workspace(directory.path()));
    let expected_version = metadata_version(&workspace, &path).await;
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();

    for contender in ["writer-a", "writer-b"] {
        let workspace = workspace.clone();
        let path = path.clone();
        let expected_version = expected_version.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            tokio::runtime::Handle::current().block_on(workspace.write_text_atomic(
                &OperationContext::new(contender, contender, "fs_write_text"),
                &path,
                contender,
                AtomicWriteOptions {
                    overwrite: true,
                    expected_version: Some(expected_version),
                    ..AtomicWriteOptions::default()
                },
            ))
        }));
    }
    barrier.wait();
    let first = tasks.remove(0).await.expect("first writer task");
    let second = tasks.remove(0).await.expect("second writer task");
    let success_count = usize::from(first.is_ok()) + usize::from(second.is_ok());
    assert_eq!(success_count, 1, "exactly one versioned writer may commit");
    let content = std::fs::read_to_string(path).expect("read race winner");
    assert!(matches!(content.as_str(), "writer-a" | "writer-b"));
}

#[tokio::test]
async fn search_stops_at_file_budget_in_generated_tree() {
    let directory = tempfile::tempdir().expect("temp directory");
    write_tree(directory.path(), 512, 2);
    let workspace = workspace(directory.path());
    let request = FsSearchRequest {
        path: directory.path().to_path_buf(),
        query: "missing-value".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: false,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 1,
        limit: 1,
        max_snippet_bytes: 128,
        budget: FsSearchBudget {
            timeout_ms: 30_000,
            max_files_scanned: 32,
            max_bytes_scanned: 1024 * 1024,
            max_output_bytes: 1024,
            max_file_bytes: 1024,
        },
    };
    let (page, continuation) = workspace
        .search_v2(
            &OperationContext::new("tree-budget", "agent", "fs_search"),
            &request,
            None,
            None,
            |_| {},
        )
        .await
        .expect("bounded search");

    assert_eq!(page.files_scanned, 32);
    assert!(page.bytes_scanned <= request.budget.max_bytes_scanned);
    assert!(page.truncation_reason.is_some());
    assert!(continuation.is_some());
}

#[tokio::test]
async fn git_status_spills_large_output_without_exceeding_inline_cap() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("run git init");
    assert!(status.success());
    for index in 0..256 {
        std::fs::write(
            directory
                .path()
                .join(format!("untracked-{index:04}-long-name.txt")),
            b"content\n",
        )
        .expect("write git fixture");
    }
    let service = GitService::new(workspace(directory.path()), 1024);
    let options = GitRunOptions {
        max_output_bytes: 1024,
        max_stderr_bytes: 1024,
        artifact_max_bytes: 1024 * 1024,
        ..GitRunOptions::default()
    };
    let output = service
        .status_with_options(directory.path(), &options, CancellationToken::new())
        .await
        .expect("bounded git status");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.len() <= options.max_output_bytes);
    assert!(output.stdout_bytes > output.stdout.len() as u64);
    assert!(output.truncated);
    assert!(output.artifact_ref.is_some());
    assert!(output.artifact_sha256.is_some());
}

#[test]
fn subprocess_staged_write_helper() {
    let Some(root) = std::env::var_os("CHATCMD_CRASH_HELPER_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::write(root.join(".chatcmd-stage-crash"), b"new-but-unpublished")
        .expect("write staged crash fixture");
    println!("READY_AFTER_STAGE");
    std::io::stdout().flush().expect("flush crash marker");
    let mut byte = [0_u8; 1];
    let _ = std::io::stdin().read(&mut byte);
}

#[test]
fn killed_subprocess_never_publishes_partial_staged_content() {
    let directory = tempfile::tempdir().expect("temp directory");
    let target = directory.path().join("target.txt");
    std::fs::write(&target, b"old-complete").expect("seed crash target");
    let child = spawn_test_helper("subprocess_staged_write_helper", directory.path());
    let status = kill_at_marker(child, "READY_AFTER_STAGE");

    assert!(!status.success());
    assert_eq!(
        std::fs::read(&target).expect("read crash target"),
        b"old-complete"
    );
    let staged = directory.path().join(".chatcmd-stage-crash");
    assert_eq!(
        std::fs::read(&staged).expect("read staged data"),
        b"new-but-unpublished"
    );
    std::fs::remove_file(staged).expect("clean staged crash fixture");
}
