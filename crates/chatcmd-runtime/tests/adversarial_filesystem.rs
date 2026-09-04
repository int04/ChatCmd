mod support;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chatcmd_runtime::{
    ApplyEditsBudget, ApplyEditsRequest, AtomicWriteOptions, BlobBeginRequest, BlobChunkRequest,
    BlobSealRequest, BlobStore, EditCoordinateSystem, FsSearchBudget, FsSearchRequest,
    FsStatBudget, FsStatRequest, GitRunOptions, GitService, GitStructuredOutput, OperationContext,
    SearchMode, TextEdit, TextReadBudget, TextReadRange, TextReadRequestV2, VersionStrength,
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read as _, Write as _},
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
    time::Instant,
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `usage` points to valid writable storage for `getrusage`, and the value is
    // assumed initialized only when the OS reports success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(status, 0, "getrusage failed");
    // SAFETY: successful `getrusage` initializes the complete `rusage` structure.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    {
        u64::try_from(usage.ru_maxrss).unwrap_or(u64::MAX)
    }
    #[cfg(target_os = "linux")]
    {
        u64::try_from(usage.ru_maxrss)
            .unwrap_or(u64::MAX)
            .saturating_mul(1024)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peak_rss_bytes() -> u64 {
    0
}

#[tokio::test]
#[ignore = "manual Plan 10 benchmark: uploads and commits 10 MiB, 100 MiB and 1 GiB blobs"]
async fn blob_upload_commit_reports_peak_rss_throughput_and_cleanup() {
    const MIB: u64 = 1024 * 1024;
    const CHUNK_BYTES: usize = MIB as usize;
    let directory = tempfile::tempdir().expect("benchmark directory");
    let workspace = workspace(directory.path());
    let blob_root = directory.path().join("blob-store");
    let store = BlobStore::new(blob_root.clone()).expect("blob store");
    let mut context = OperationContext::new("plan10-benchmark", "agent", "fs_write_raw");
    context.task_id = Some("plan10-benchmark-task".into());
    context.turn_id = Some("plan10-benchmark-turn".into());
    let chunk = vec![0x5a_u8; CHUNK_BYTES];
    let encoded = STANDARD.encode(&chunk);

    for size in [10 * MIB, 100 * MIB, 1024 * MIB] {
        let chunks = size / MIB;
        let mut hasher = Sha256::new();
        for _ in 0..chunks {
            hasher.update(&chunk);
        }
        let final_hash = format!("{:x}", hasher.finalize());
        let rss_before = peak_rss_bytes();
        let started = Instant::now();
        let begin = store
            .begin(
                &context,
                BlobBeginRequest {
                    purpose: "fsWriteRaw".into(),
                    expected_size_bytes: Some(size),
                    content_type: Some("application/octet-stream".into()),
                    expected_sha256: Some(final_hash.clone()),
                    chunk_size_bytes: Some(CHUNK_BYTES),
                    ttl_seconds: None,
                    budget: Default::default(),
                },
            )
            .expect("begin benchmark blob");
        for index in 0..chunks {
            store
                .write_chunk(
                    &context,
                    BlobChunkRequest {
                        upload_id: begin.upload_id.clone(),
                        offset: index * MIB,
                        data_base64: encoded.clone(),
                        chunk_sha256: None,
                        budget: Default::default(),
                    },
                )
                .expect("write benchmark chunk");
        }
        store
            .seal(
                &context,
                BlobSealRequest {
                    upload_id: begin.upload_id.clone(),
                    final_size_bytes: size,
                    sha256: final_hash,
                    budget: Default::default(),
                },
            )
            .expect("seal benchmark blob");
        let temp_disk_bytes =
            std::fs::metadata(blob_root.join(format!("{}.blob", begin.upload_id)))
                .expect("blob metadata")
                .len();
        let lease = store
            .lease(&context, &begin.content_ref, "fsWriteRaw")
            .expect("lease benchmark blob");
        let target = directory.path().join(format!("blob-commit-{size}.bin"));
        workspace
            .write_blob_atomic(
                &context,
                &target,
                lease.path(),
                AtomicWriteOptions::default(),
                false,
            )
            .await
            .expect("commit benchmark blob");
        let cleanup_started = Instant::now();
        lease.finish(true).expect("consume benchmark blob");
        let cleanup_elapsed = cleanup_started.elapsed();
        let elapsed = started.elapsed();
        let peak_rss = peak_rss_bytes();
        let rss_growth = peak_rss.saturating_sub(rss_before);
        let throughput_mib_s = (size as f64 / MIB as f64) / elapsed.as_secs_f64();
        let request_count = chunks + 3;
        println!(
            "PLAN10_BENCH size_mib={} elapsed_ms={} throughput_mib_s={:.2} peak_rss_mib={:.2} rss_growth_mib={:.2} requests={} temp_disk_mib={:.2} cleanup_ms={}",
            size / MIB,
            elapsed.as_millis(),
            throughput_mib_s,
            peak_rss as f64 / MIB as f64,
            rss_growth as f64 / MIB as f64,
            request_count,
            temp_disk_bytes as f64 / MIB as f64,
            cleanup_elapsed.as_millis()
        );
        assert_eq!(
            std::fs::metadata(&target).expect("target metadata").len(),
            size
        );
        assert!(!blob_root.join(format!("{}.blob", begin.upload_id)).exists());
        if peak_rss != 0 {
            assert!(
                rss_growth < 128 * MIB,
                "blob upload RSS growth exceeded 128 MiB: {rss_growth} bytes"
            );
        }
        std::fs::remove_file(target).expect("remove benchmark target");
    }
}

#[tokio::test]
#[ignore = "manual Plan 09 benchmark: streams 100 MiB and 1 GiB files"]
async fn apply_edits_100mib_and_1gib_reports_peak_rss_and_throughput() {
    const MIB: u64 = 1024 * 1024;
    let directory = tempfile::tempdir().expect("benchmark directory");
    let workspace = workspace(directory.path());

    for size in [100 * MIB, 1024 * MIB] {
        let path = directory.path().join(format!("apply-edits-{size}.txt"));
        let file = std::fs::File::create(&path).expect("create sparse benchmark file");
        file.set_len(size).expect("size sparse benchmark file");
        let expected_version = metadata_version(&workspace, &path).await;
        let request = ApplyEditsRequest {
            path: path.clone(),
            expected_version,
            coordinate_system: EditCoordinateSystem::Byte,
            column_encoding: None,
            edits: vec![TextEdit {
                start_byte: Some(size / 2),
                end_byte: Some(size / 2 + 1),
                start: None,
                end: None,
                text: "x".to_owned(),
            }],
            dry_run: false,
            preserve_line_endings: true,
            preserve_bom: true,
            budget: ApplyEditsBudget {
                timeout_ms: 5 * 60_000,
                max_bytes_read: 2 * 1024 * MIB,
                max_bytes_written: 2 * 1024 * MIB,
                max_edits: 16,
            },
        };
        let rss_before = peak_rss_bytes();
        let started = Instant::now();
        let result = workspace
            .apply_edits(
                &OperationContext::new("plan09-benchmark", "agent", "fs_apply_edits"),
                &request,
            )
            .await
            .expect("apply benchmark edit");
        let elapsed = started.elapsed();
        let peak_rss = peak_rss_bytes();
        let rss_growth = peak_rss.saturating_sub(rss_before);
        let processed = result.bytes_read.saturating_add(result.bytes_written);
        let throughput_mib_s = processed as f64 / MIB as f64 / elapsed.as_secs_f64();

        println!(
            "PLAN09_BENCH size_mib={} elapsed_ms={} throughput_mib_s={:.2} peak_rss_mib={:.2} rss_growth_mib={:.2}",
            size / MIB,
            elapsed.as_millis(),
            throughput_mib_s,
            peak_rss as f64 / MIB as f64,
            rss_growth as f64 / MIB as f64
        );
        assert!(result.applied);
        assert_eq!(std::fs::metadata(&path).expect("metadata").len(), size);
        if peak_rss != 0 {
            assert!(
                rss_growth < 128 * MIB,
                "fs_apply_edits RSS growth exceeded 128 MiB: {rss_growth} bytes"
            );
        }
    }
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

#[tokio::test]
async fn git_commit_success_returns_structured_commit_hash() {
    let directory = tempfile::tempdir().expect("temp directory");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(directory.path())
            .status()
            .expect("configure git fixture");
        assert!(status.success());
    }
    std::fs::write(directory.path().join("tracked.txt"), b"content\n").expect("write fixture");
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .commit_with_options(
            directory.path(),
            "successful commit",
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("successful bounded commit");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    match output.structured {
        Some(GitStructuredOutput::Commit(data)) => {
            assert_eq!(data.phase, "commitHooksIncluded");
            assert!(data.hooks_included);
            let hash = data.commit_hash.expect("commit hash");
            assert_eq!(hash.len(), 40);
            assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        other => panic!("expected structured commit metadata, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn git_commit_hanging_pre_commit_hook_times_out_and_is_reaped() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temp directory");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(directory.path())
            .status()
            .expect("configure git fixture");
        assert!(status.success());
    }
    std::fs::write(directory.path().join("tracked.txt"), b"content\n").expect("write fixture");
    let hook = directory.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, b"#!/bin/sh\nsleep 30\n").expect("write hanging hook");
    let mut permissions = std::fs::metadata(&hook)
        .expect("hook metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).expect("make hook executable");

    let service = GitService::new(workspace(directory.path()), 4096);
    let options = GitRunOptions {
        timeout_ms: 300,
        max_runtime_ms: 300,
        ..GitRunOptions::default()
    };
    let output = service
        .commit_with_options(
            directory.path(),
            "must time out",
            true,
            &[],
            &options,
            CancellationToken::new(),
        )
        .await
        .expect("bounded commit timeout");

    assert!(output.timed_out, "hanging hook must obey git timeout");
    assert!(!output.cancelled);
    assert!(
        output.elapsed_ms < 5_000,
        "hook process tree was not reaped promptly"
    );
    let count = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(directory.path())
        .output()
        .expect("inspect commit result");
    assert!(
        !count.status.success(),
        "timed-out hook must not create a commit"
    );
}

#[tokio::test]
async fn git_corrupt_repository_and_index_lock_fail_without_panicking() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    std::fs::write(directory.path().join(".git/HEAD"), b"not-a-valid-head\n")
        .expect("corrupt HEAD");
    let service = GitService::new(workspace(directory.path()), 4096);
    let status_output = service
        .status_with_options(
            directory.path(),
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("corrupt repository returns process outcome");
    assert_ne!(status_output.exit_code, Some(0));

    let locked = tempfile::tempdir().expect("locked repository");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(locked.path())
            .status()
            .expect("configure locked repo");
        assert!(status.success());
    }
    std::fs::write(locked.path().join("tracked.txt"), b"content\n").expect("write fixture");
    std::fs::write(locked.path().join(".git/index.lock"), b"locked\n").expect("create index lock");
    let service = GitService::new(workspace(locked.path()), 4096);
    let output = service
        .commit_with_options(
            locked.path(),
            "must fail during stage",
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("locked index returns bounded outcome");
    assert_ne!(output.exit_code, Some(0));
    match output.structured {
        Some(GitStructuredOutput::Commit(data)) => assert_eq!(data.phase, "stage"),
        other => panic!("expected stage failure metadata, got {other:?}"),
    }
}

#[tokio::test]
async fn git_binary_diff_remains_bounded_and_argument_safe() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let binary = (0_u8..=255)
        .cycle()
        .take(2 * 1024 * 1024)
        .collect::<Vec<_>>();
    std::fs::write(directory.path().join("binary.dat"), binary).expect("write binary fixture");
    let status = Command::new("git")
        .args(["add", "--", "binary.dat"])
        .current_dir(directory.path())
        .status()
        .expect("stage binary fixture");
    assert!(status.success());
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .diff_with_options(
            directory.path(),
            true,
            false,
            None,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("bounded binary diff");
    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout_bytes < 64 * 1024);
    assert!(output.stdout.contains("Binary files") || output.stdout.contains("GIT binary patch"));
}

#[cfg(unix)]
#[tokio::test]
async fn git_diff_disables_configured_external_diff() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let marker = directory.path().join("external-diff-ran");
    let script = directory.path().join("external-diff.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\nsleep 30\n", marker.display()),
    )
    .expect("write external diff helper");
    let mut permissions = std::fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make helper executable");
    let status = Command::new("git")
        .args([
            "config",
            "diff.external",
            script.to_str().expect("script path"),
        ])
        .current_dir(directory.path())
        .status()
        .expect("configure external diff");
    assert!(status.success());
    std::fs::write(directory.path().join("file.txt"), b"content\n").expect("write fixture");
    let status = Command::new("git")
        .args(["add", "--", "file.txt"])
        .current_dir(directory.path())
        .status()
        .expect("stage fixture");
    assert!(status.success());
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .diff_with_options(
            directory.path(),
            true,
            false,
            None,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("diff with external diff disabled");
    assert_eq!(output.exit_code, Some(0));
    assert!(
        !marker.exists(),
        "--no-ext-diff must suppress configured helper"
    );
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
