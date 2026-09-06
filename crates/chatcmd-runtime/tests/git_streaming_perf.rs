#![allow(dead_code)]

mod support;

use chatcmd_runtime::{GitRunOptions, GitService, GitStructuredOutput};
use std::{
    fs::File,
    io::{BufWriter, Write as _},
    process::Command,
    time::Instant,
};
use support::workspace;
use tokio_util::sync::CancellationToken;

const MIB: u64 = 1024 * 1024;

fn init_repo(path: &std::path::Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(path)
        .status()
        .expect("git init");
    assert!(status.success());
    let status = Command::new("git")
        .args(["config", "core.bigFileThreshold", "2g"])
        .current_dir(path)
        .status()
        .expect("configure large text diff threshold");
    assert!(status.success());
}

fn write_repeated_text(path: &std::path::Path, bytes: u64) {
    let file = File::create(path).expect("create benchmark fixture");
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    let chunk = b"chatcmd-git-streaming-benchmark-line-0123456789abcdef\n".repeat(16 * 1024);
    let mut remaining = bytes;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(chunk.len() as u64)).expect("chunk size");
        writer
            .write_all(&chunk[..count])
            .expect("write fixture chunk");
        remaining -= count as u64;
    }
    writer.flush().expect("flush fixture");
}

fn stage(path: &std::path::Path, file: &str) {
    let status = Command::new("git")
        .args(["add", "--", file])
        .current_dir(path)
        .status()
        .expect("git add benchmark fixture");
    assert!(status.success());
}

fn write_and_stage_diff_fixture(path: &std::path::Path, total_bytes: u64) {
    let folder = path.join("generated");
    std::fs::create_dir(&folder).expect("create diff fixture folder");
    let mut remaining = total_bytes;
    let mut index = 0_u32;
    while remaining > 0 {
        let bytes = remaining.min(64 * MIB);
        write_repeated_text(&folder.join(format!("part-{index:04}.txt")), bytes);
        remaining -= bytes;
        index += 1;
    }
    stage(path, "generated");
}

fn benchmark_options() -> GitRunOptions {
    GitRunOptions {
        max_output_bytes: 64 * 1024,
        max_stderr_bytes: 64 * 1024,
        artifact_max_bytes: 1024 * MIB,
        timeout_ms: 10 * 60 * 1000,
        max_runtime_ms: 10 * 60 * 1000,
        limit: 200,
        ..GitRunOptions::default()
    }
}

#[tokio::test]
#[ignore = "manual Plan 15 benchmark: staged text diff at 10 MiB, 100 MiB and 1 GiB"]
async fn git_diff_10mib_100mib_1gib_reports_streaming_metrics() {
    for size_mib in [10_u64, 100, 1024] {
        let directory = tempfile::tempdir().expect("temp directory");
        init_repo(directory.path());
        write_and_stage_diff_fixture(directory.path(), size_mib * MIB);

        let service = GitService::new(workspace(directory.path()), 64 * 1024);
        let started = Instant::now();
        let output = service
            .diff_with_options(
                directory.path(),
                true,
                false,
                None,
                &benchmark_options(),
                CancellationToken::new(),
            )
            .await
            .expect("bounded staged diff");
        let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let throughput_mib_s = if output.elapsed_ms == 0 {
            0.0
        } else {
            output.artifact_bytes as f64 / MIB as f64 / (output.elapsed_ms as f64 / 1000.0)
        };
        eprintln!(
            "PLAN15_DIFF sizeMiB={size_mib} stdoutBytes={} artifactBytes={} firstOutputMs={:?} elapsedMs={} wallMs={} artifactMiBPerSec={throughput_mib_s:.2} truncated={} reason={:?}",
            output.stdout_bytes,
            output.artifact_bytes,
            output.first_output_ms,
            output.elapsed_ms,
            wall_ms,
            output.truncated,
            output.truncation_reason
        );
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout_bytes >= size_mib * MIB);
        assert!(output.stdout.len() <= 64 * 1024);
        assert!(output.first_output_ms.is_some());
        assert!(output.truncated);
    }
}

#[tokio::test]
#[ignore = "manual Plan 15 benchmark: Git status over 100,000 untracked entries"]
async fn git_status_100k_entries_reports_first_page_and_metrics() {
    let directory = tempfile::tempdir().expect("temp directory");
    init_repo(directory.path());
    for bucket in 0..100_u32 {
        let folder = directory.path().join(format!("bucket-{bucket:03}"));
        std::fs::create_dir(&folder).expect("create bucket");
        for item in 0..1000_u32 {
            std::fs::write(folder.join(format!("entry-{item:04}.txt")), b"x\n")
                .expect("write status fixture");
        }
    }

    let service = GitService::new(workspace(directory.path()), 64 * 1024);
    let options = GitRunOptions {
        limit: 200,
        max_output_bytes: 64 * 1024,
        artifact_max_bytes: 128 * MIB,
        timeout_ms: 10 * 60 * 1000,
        max_runtime_ms: 10 * 60 * 1000,
        ..GitRunOptions::default()
    };
    let output = service
        .status_with_options(directory.path(), &options, CancellationToken::new())
        .await
        .expect("100k git status");
    eprintln!(
        "PLAN15_STATUS entries=100000 stdoutBytes={} artifactBytes={} firstOutputMs={:?} elapsedMs={} truncated={} reason={:?}",
        output.stdout_bytes,
        output.artifact_bytes,
        output.first_output_ms,
        output.elapsed_ms,
        output.truncated,
        output.truncation_reason
    );
    assert_eq!(output.exit_code, Some(0));
    match output.structured {
        Some(GitStructuredOutput::Status(data)) => {
            assert_eq!(data.entries.len(), 200);
            assert!(data.has_more);
            assert!(data.next_cursor.is_some());
        }
        other => panic!("expected structured status, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "manual Plan 15 benchmark: cancellation latency during large staged diff"]
async fn git_diff_cancellation_latency_is_bounded() {
    let directory = tempfile::tempdir().expect("temp directory");
    init_repo(directory.path());
    write_repeated_text(&directory.path().join("cancel.txt"), 256 * MIB);
    stage(directory.path(), "cancel.txt");
    let service = GitService::new(workspace(directory.path()), 64 * 1024);
    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let output = service
        .diff_with_options(
            directory.path(),
            true,
            false,
            None,
            &benchmark_options(),
            cancellation,
        )
        .await
        .expect("cancelled staged diff");
    let total_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let cancellation_latency_ms = total_ms.saturating_sub(50);
    eprintln!(
        "PLAN15_CANCEL totalMs={total_ms} cancelLatencyMs={cancellation_latency_ms} stdoutBytes={} firstOutputMs={:?}",
        output.stdout_bytes, output.first_output_ms
    );
    assert!(output.cancelled);
    assert!(cancellation_latency_ms < 5_000);
}
