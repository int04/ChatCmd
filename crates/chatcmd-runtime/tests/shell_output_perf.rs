use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, NullEventSink, OperationContext, PolicyContext,
    PolicyDecision, PolicyEngine, RuntimeConfig, RuntimeResult, ShellCreateRequest, ShellRuntime,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
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

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis())
}

fn percentile(values: &mut [u128], percentile: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = ((values.len() - 1) * percentile / 100).min(values.len() - 1);
    values[index]
}

#[cfg(unix)]
fn process_usage() -> (f64, u64) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage structure on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return (0.0, 0);
    }
    // SAFETY: getrusage returned success, so the structure is initialized.
    let usage = unsafe { usage.assume_init() };
    let seconds = usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1_000_000.0
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1_000_000.0;
    #[cfg(target_os = "macos")]
    let peak_rss_bytes = u64::try_from(usage.ru_maxrss).unwrap_or(0);
    #[cfg(not(target_os = "macos"))]
    let peak_rss_bytes = u64::try_from(usage.ru_maxrss)
        .unwrap_or(0)
        .saturating_mul(1024);
    (seconds, peak_rss_bytes)
}

#[cfg(not(unix))]
fn process_usage() -> (f64, u64) {
    (0.0, 0)
}

fn runtime(root: &Path) -> ShellRuntime {
    ShellRuntime::new(
        RuntimeConfig {
            roots: vec![root.to_path_buf()],
            max_sessions: 4,
            max_replay_bytes: 8 * 1024 * 1024,
            max_replay_events: 8_192,
            shell_output_chunk_bytes: 16 * 1024,
            shell_output_max_latency_ms: 25,
            ..RuntimeConfig::default()
        },
        policy(),
        Arc::new(NullEventSink),
    )
}

fn create_request(root: PathBuf, request_id: &str, command: String) -> ShellCreateRequest {
    ShellCreateRequest {
        request_id: request_id.to_owned(),
        working_directory: Some(root),
        executable: Some(PathBuf::from("/bin/sh")),
        arguments: vec!["-c".to_owned(), command],
        environment: BTreeMap::new(),
        columns: Some(120),
        rows: Some(30),
    }
}

async fn run_output_case(
    runtime: &ShellRuntime,
    root: &Path,
    name: &str,
    bytes: usize,
    poll_delay: Option<Duration>,
) -> serde_json::Value {
    let marker = format!("__PLAN22_{name}_DONE__");
    let command = format!("head -c {bytes} /dev/zero | tr '\\0' x; printf '{marker}\\n'");
    let (cpu_before, _) = process_usage();
    let started = Instant::now();
    let created = runtime
        .create(
            &OperationContext::new(format!("{name}-create"), "plan22-bench", "shell_create"),
            create_request(root.to_path_buf(), &format!("{name}-create"), command),
        )
        .await
        .expect("create benchmark shell");

    if poll_delay.is_none() {
        let waited = runtime
            .wait(&created.session_id, Duration::from_secs(120))
            .await
            .expect("wait for no-consumer producer");
        assert!(waited.completed, "no-consumer producer did not exit");
    }

    let mut cursor = 0_u64;
    let mut latencies_ms = Vec::new();
    let mut retained_bytes = 0_u64;
    let mut replay_truncated = false;
    let mut latest_sequence = 0_u64;
    let mut read_calls = 0_u64;
    let mut saw_marker = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while !saw_marker {
        read_calls = read_calls.saturating_add(1);
        let (result, _) = runtime
            .read_with_context(
                &OperationContext::new(
                    format!("{name}-read-{cursor}"),
                    "plan22-bench",
                    "shell_read",
                ),
                &created.session_id,
                cursor,
                2_000,
            )
            .await
            .expect("read benchmark shell");
        replay_truncated |= result.replay_truncated;
        latest_sequence = latest_sequence.max(result.latest_available_sequence);
        for event in &result.events {
            cursor = cursor.max(event.sequence);
            retained_bytes = retained_bytes.saturating_add(event.data.len() as u64);
            latencies_ms.push(now_ms().saturating_sub(event.timestamp_unix_ms));
            if event.data.contains(&marker) {
                saw_marker = true;
            }
        }
        assert!(
            Instant::now() < deadline,
            "benchmark producer did not finish: {name}"
        );
        if !saw_marker {
            tokio::time::sleep(poll_delay.unwrap_or(Duration::from_millis(10))).await;
        }
    }
    let elapsed = started.elapsed();
    let (cpu_after, peak_rss_bytes) = process_usage();
    let mut latency_copy = latencies_ms.clone();
    let p50_ms = percentile(&mut latency_copy, 50);
    let p95_ms = percentile(&mut latencies_ms, 95);
    let throughput_mib_s = bytes as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
    let baseline_events_at_8k_reads = bytes.div_ceil(8 * 1024);
    let event_reduction = if latest_sequence == 0 {
        0.0
    } else {
        baseline_events_at_8k_reads as f64 / latest_sequence as f64
    };
    runtime
        .close(
            &OperationContext::new(format!("{name}-close"), "plan22-bench", "shell_close"),
            &created.session_id,
            true,
        )
        .await
        .expect("close benchmark shell");

    json!({
        "case": name,
        "producedBytes": bytes,
        "elapsedMs": elapsed.as_millis(),
        "throughputMiBPerSec": throughput_mib_s,
        "coalescedEvents": latest_sequence,
        "shellReadCalls": read_calls,
        "baselineEventsAt8KiBRawReads": baseline_events_at_8k_reads,
        "eventReductionFactor": event_reduction,
        "replayTruncated": replay_truncated,
        "retainedPayloadBytesObserved": retained_bytes,
        "deliveryLatencyP50Ms": p50_ms,
        "deliveryLatencyP95Ms": p95_ms,
        "cpuSeconds": (cpu_after - cpu_before).max(0.0),
        "peakProcessRssBytes": peak_rss_bytes,
    })
}

async fn run_stop_case(runtime: &ShellRuntime, root: &Path) -> serde_json::Value {
    let created = runtime
        .create(
            &OperationContext::new("stop-create", "plan22-bench", "shell_create"),
            create_request(
                root.to_path_buf(),
                "stop-create",
                "while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done"
                    .to_owned(),
            ),
        )
        .await
        .expect("create stop benchmark shell");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    runtime
        .close(
            &OperationContext::new("stop-close", "plan22-bench", "shell_close"),
            &created.session_id,
            true,
        )
        .await
        .expect("force-close benchmark shell");
    json!({ "forceStopLatencyMs": started.elapsed().as_millis() })
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual Plan 22 PTY throughput/backpressure/resource benchmark"]
async fn plan22_pty_output_benchmark() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path());
    let bytes = std::env::var("CHATCMD_PLAN22_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100 * 1024 * 1024);

    let fast = run_output_case(
        &runtime,
        directory.path(),
        "fast",
        bytes,
        Some(Duration::from_millis(1)),
    )
    .await;
    let slow = run_output_case(
        &runtime,
        directory.path(),
        "slow",
        bytes,
        Some(Duration::from_millis(50)),
    )
    .await;
    let no_consumer = run_output_case(&runtime, directory.path(), "no_consumer", bytes, None).await;
    let stop = run_stop_case(&runtime, directory.path()).await;

    println!(
        "PLAN22_BENCHMARK={}",
        json!({
            "chunkBytes": 16 * 1024,
            "maxLatencyMs": 25,
            "replayBytes": 8 * 1024 * 1024,
            "replayEvents": 8_192,
            "fastConsumer": fast,
            "slowConsumer": slow,
            "noConsumer": no_consumer,
            "stop": stop,
        })
    );
}
