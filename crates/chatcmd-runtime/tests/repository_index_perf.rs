use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, FindPatternMode, FsBatchStatRequest, FsFindRequest,
    OperationContext, PolicyContext, PolicyEngine, RuntimeResult, WorkspaceService,
};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

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

fn fixture_count() -> usize {
    std::env::var("CHATCMD_PLAN20_PATHS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000)
}

fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
    samples.sort_unstable();
    let index = samples.len().saturating_sub(1).saturating_mul(percentile) / 100;
    samples[index]
}

fn create_fixture(root: &Path, file_count: usize) {
    for directory in 0..100_usize {
        std::fs::create_dir_all(root.join(format!("d{directory:03}"))).expect("create directory");
    }
    for index in 0..file_count {
        let path = root
            .join(format!("d{:03}", index % 100))
            .join(format!("file-{index:07}.txt"));
        let content: &[u8] = if index % 997 == 0 { b"needle" } else { b"data" };
        std::fs::write(path, content).expect("write fixture");
    }
}

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage structure on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: the previous getrusage call succeeded.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    {
        u64::try_from(usage.ru_maxrss).unwrap_or(0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        u64::try_from(usage.ru_maxrss)
            .unwrap_or(0)
            .saturating_mul(1024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}

#[tokio::test]
#[ignore = "manual Plan 20 benchmark: set CHATCMD_PLAN20_PATHS=100000 or 1000000"]
async fn repository_index_cold_warm_incremental_and_batch_benchmark() {
    let requested_paths = fixture_count().clamp(101, 1_000_000);
    let file_count = requested_paths.saturating_sub(101);
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture_started = Instant::now();
    create_fixture(temp.path(), file_count);
    let fixture_elapsed = fixture_started.elapsed();
    let workspace = service(temp.path());

    let rebuild = OperationContext::new("bench-index", "benchmark", "workspace_index_rebuild");
    let cold_started = Instant::now();
    let status = workspace
        .rebuild_index(&rebuild, temp.path())
        .await
        .expect("cold rebuild");
    let cold_elapsed = cold_started.elapsed();

    let request = FsFindRequest {
        path: temp.path().to_path_buf(),
        pattern: format!("file-{:07}", file_count.saturating_sub(1)),
        pattern_mode: FindPatternMode::Literal,
        case_sensitive: true,
        entry_types: Vec::new(),
        max_depth: 64,
        include_ignored: false,
        include_hidden: false,
        exclude: Vec::new(),
        extensions: vec!["txt".to_owned()],
        limit: 200,
        budget: Default::default(),
    };
    let mut warm_samples = Vec::with_capacity(30);
    for sample in 0..30 {
        let started = Instant::now();
        let (page, _) = workspace
            .find_v2(
                &OperationContext::new(format!("warm-{sample}"), "benchmark", "fs_find"),
                &request,
                None,
                None,
            )
            .await
            .expect("warm indexed find");
        assert!(page.data.index_used);
        warm_samples.push(started.elapsed());
    }
    let warm_p50 = percentile(&mut warm_samples.clone(), 50);
    let warm_p95 = percentile(&mut warm_samples, 95);

    let direct_workspace = service(temp.path());
    let mut direct_samples = Vec::with_capacity(10);
    for sample in 0..10 {
        let started = Instant::now();
        let (page, _) = direct_workspace
            .find_v2(
                &OperationContext::new(format!("direct-{sample}"), "benchmark", "fs_find"),
                &request,
                None,
                None,
            )
            .await
            .expect("direct find");
        assert!(!page.data.index_used);
        direct_samples.push(started.elapsed());
    }
    let direct_p50 = percentile(&mut direct_samples.clone(), 50);
    let direct_p95 = percentile(&mut direct_samples, 95);

    let batch_paths = (0..500_usize)
        .map(|index| {
            temp.path()
                .join(format!("d{:03}", index % 100))
                .join(format!("file-{index:07}.txt"))
        })
        .collect::<Vec<_>>();
    let stat_context = OperationContext::new("bench-batch", "benchmark", "fs_batch_stat");
    let batch_started = Instant::now();
    let batch = workspace
        .batch_stat(
            &stat_context,
            &FsBatchStatRequest {
                paths: batch_paths.clone(),
                version_strength: Default::default(),
                max_items: 500,
                budget: Default::default(),
            },
        )
        .await
        .expect("batch stat");
    let batch_elapsed = batch_started.elapsed();
    assert_eq!(batch.usage.succeeded, 500);
    let sequential_started = Instant::now();
    for path in &batch_paths {
        workspace.stat(path).await.expect("sequential stat");
    }
    let sequential_elapsed = sequential_started.elapsed();

    for changes in [1_usize, 100, 10_000.min(file_count)] {
        workspace
            .rebuild_index(&rebuild, temp.path())
            .await
            .expect("reset fresh index");
        let started = Instant::now();
        for index in 0..changes {
            let path = temp
                .path()
                .join(format!("d{:03}", index % 100))
                .join(format!("file-{index:07}.txt"));
            std::fs::write(&path, b"changed-data").expect("external update");
            workspace.mark_index_stale(&path);
        }
        eprintln!(
            "plan20 incremental changes={changes} elapsed_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    eprintln!(
        "plan20 requested_paths={requested_paths} entries={} fixture_ms={:.3} cold_ms={:.3} warm_p50_ms={:.3} warm_p95_ms={:.3} direct_p50_ms={:.3} direct_p95_ms={:.3} batch500_ms={:.3} sequential500_ms={:.3} peak_rss_bytes={}",
        status.entry_count,
        fixture_elapsed.as_secs_f64() * 1000.0,
        cold_elapsed.as_secs_f64() * 1000.0,
        warm_p50.as_secs_f64() * 1000.0,
        warm_p95.as_secs_f64() * 1000.0,
        direct_p50.as_secs_f64() * 1000.0,
        direct_p95.as_secs_f64() * 1000.0,
        batch_elapsed.as_secs_f64() * 1000.0,
        sequential_elapsed.as_secs_f64() * 1000.0,
        peak_rss_bytes(),
    );
}
