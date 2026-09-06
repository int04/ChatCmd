use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, PolicyDecision, PolicyEngine, RuntimeResult,
    TextReadBudget, TextReadRange, TextReadRequestV2, WorkspaceService,
};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
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

fn make_lines(path: &Path, size: usize) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    let line = [b'x'; 127];
    let mut written = 0;
    while written < size {
        writer.write_all(&line).unwrap();
        writer.write_all(b"\n").unwrap();
        written += 128;
    }
    writer.flush().unwrap();
}

async fn run_case(ws: &WorkspaceService, path: PathBuf, label: &str, range: TextReadRange) {
    let request = TextReadRequestV2 {
        path,
        range,
        max_bytes: 4096,
        include_line_endings: true,
        expected_version: None,
        budget: TextReadBudget {
            timeout_ms: 60_000,
            max_bytes_read: 256 * 1024 * 1024,
        },
    };
    let started = Instant::now();
    let result = ws.read_text_v2(None, &request).await.unwrap();
    println!(
        "PERF {label} wall_ms={} bytes_read={} output_bytes={} allocation_upper_bytes={} size_bytes={}",
        started.elapsed().as_millis(),
        result.bytes_read,
        result.content.len(),
        64 * 1024 + request.max_bytes + 3,
        result.size_bytes
    );
    assert!(result.content.len() <= request.max_bytes);
    assert!(result.bytes_read <= request.budget.max_bytes_read);
}

#[tokio::test]
#[ignore = "manual Plan 03 large-file performance fixture"]
async fn large_file_read_performance_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path().to_path_buf());
    for (label, size) in [("10MiB", 10 * 1024 * 1024), ("100MiB", 100 * 1024 * 1024)] {
        let path = dir.path().join(format!("{label}.txt"));
        make_lines(&path, size);
        run_case(
            &ws,
            path.clone(),
            &format!("{label}-first"),
            TextReadRange::Line {
                start: 1,
                limit: 16,
            },
        )
        .await;
        let deep_line = size / 128 - 16;
        run_case(
            &ws,
            path,
            &format!("{label}-deep"),
            TextReadRange::Line {
                start: deep_line,
                limit: 16,
            },
        )
        .await;
    }

    let sparse = dir.path().join("1GiB-sparse.txt");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&sparse)
        .unwrap();
    file.write_all(b"head\n").unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    drop(file);
    run_case(
        &ws,
        sparse.clone(),
        "1GiB-first",
        TextReadRange::Line { start: 1, limit: 1 },
    )
    .await;
    run_case(
        &ws,
        sparse.clone(),
        "1GiB-middle-byte",
        TextReadRange::Byte {
            start: 512 * 1024 * 1024,
            limit: 4096,
        },
    )
    .await;
    run_case(
        &ws,
        sparse,
        "1GiB-end-byte",
        TextReadRange::Byte {
            start: 1024 * 1024 * 1024 - 4096,
            limit: 4096,
        },
    )
    .await;
}
