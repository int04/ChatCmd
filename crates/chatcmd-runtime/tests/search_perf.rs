use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, FsSearchBudget, FsSearchRequest,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, SearchMode, WorkspaceService,
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Approve;
impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn workspace(root: PathBuf) -> WorkspaceService {
    WorkspaceService::new(
        &[root],
        PolicyEngine::new(
            Some(ExecutionPolicy {
                default: PolicyDecision::Allow,
                per_agent_tool: BTreeMap::new(),
                per_root: BTreeMap::new(),
            }),
            Arc::new(Approve),
        ),
    )
    .expect("workspace")
}

fn request(root: &Path) -> FsSearchRequest {
    FsSearchRequest {
        path: root.to_path_buf(),
        query: "needle-that-does-not-exist".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: true,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 10,
        limit: 200,
        max_snippet_bytes: 8 * 1024,
        budget: FsSearchBudget {
            timeout_ms: 10 * 60 * 1000,
            max_files_scanned: 200_000,
            max_bytes_scanned: 256 * 1024 * 1024,
            max_output_bytes: 512 * 1024,
            max_file_bytes: 128 * 1024 * 1024,
        },
    }
}

fn make_file_tree(root: &Path, count: usize) {
    const PER_DIR: usize = 1_000;
    for dir_index in 0..count.div_ceil(PER_DIR) {
        let dir = root.join(format!("d{dir_index:04}"));
        fs::create_dir_all(&dir).expect("mkdir benchmark fixture");
        let start = dir_index * PER_DIR;
        let end = (start + PER_DIR).min(count);
        for index in start..end {
            fs::write(dir.join(format!("f{index:06}.txt")), b"alpha beta gamma\n")
                .expect("write benchmark file");
        }
    }
}

fn make_large_file(path: &Path, size: usize) {
    let file = File::create(path).expect("create large benchmark file");
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    let line = b"alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu\n";
    let mut written = 0usize;
    while written < size {
        writer.write_all(line).expect("write large benchmark line");
        written += line.len();
    }
    writer.flush().expect("flush benchmark file");
}

#[cfg(windows)]
fn peak_working_set_bytes() -> Option<u64> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(windows))]
fn peak_working_set_bytes() -> Option<u64> {
    None
}

async fn run_case(ws: &WorkspaceService, root: &Path, label: &str) {
    let emitted = Arc::new(Mutex::new((Instant::now() - Duration::from_secs(1), 0_u64)));
    let emitted_for_progress = emitted.clone();
    let started = Instant::now();
    let (page, state) = ws
        .search_v2(
            &OperationContext::new(format!("perf-{label}"), "perf", "fs_search"),
            &request(root),
            None,
            None,
            move |_| {
                let mut guard = emitted_for_progress.lock().expect("progress lock");
                if guard.0.elapsed() >= Duration::from_millis(250) {
                    guard.0 = Instant::now();
                    guard.1 += 1;
                }
            },
        )
        .await
        .expect("benchmark search");
    let elapsed = started.elapsed();
    let emitted_events = emitted.lock().expect("progress result").1;
    let mib = page.bytes_scanned as f64 / (1024.0 * 1024.0);
    let seconds = elapsed.as_secs_f64().max(0.000_001);
    println!(
        "SEARCH_PERF {label} wall_ms={} files_scanned={} bytes_scanned={} mib_per_sec={:.2} files_per_sec={:.2} peak_working_set_bytes={} ui_events={} truncated={:?}",
        elapsed.as_millis(),
        page.files_scanned,
        page.bytes_scanned,
        mib / seconds,
        page.files_scanned as f64 / seconds,
        peak_working_set_bytes().unwrap_or(0),
        emitted_events,
        page.truncation_reason
    );
    assert!(state.is_none(), "fixture must finish without continuation");
    assert!(page.truncation_reason.is_none(), "fixture must fit budgets");
    assert!(emitted_events <= elapsed.as_secs().saturating_mul(5).saturating_add(2));
}

#[tokio::test]
#[ignore = "manual Plan 06 fs_search scalability benchmark"]
async fn search_scalability_benchmark() {
    for count in [10_000usize, 100_000] {
        let dir = tempfile::tempdir().expect("tempdir");
        make_file_tree(dir.path(), count);
        run_case(
            &workspace(dir.path().to_path_buf()),
            dir.path(),
            &format!("{count}-files"),
        )
        .await;
    }

    for (label, size) in [
        ("10MiB-file", 10 * 1024 * 1024usize),
        ("100MiB-file", 100 * 1024 * 1024usize),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        make_large_file(&dir.path().join("large.txt"), size);
        run_case(&workspace(dir.path().to_path_buf()), dir.path(), label).await;
    }
}
