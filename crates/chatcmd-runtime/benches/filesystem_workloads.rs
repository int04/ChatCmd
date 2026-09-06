use chatcmd_runtime::{
    ApprovalDecision, AtomicWriteOptions, BoxFuture, ExecutionPolicy, FsSearchBudget,
    FsSearchRequest, OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, SearchMode,
    TextReadBudget, TextReadRange, TextReadRequestV2, WorkspaceService,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::{
    collections::BTreeMap,
    hint::black_box,
    io::{BufWriter, Write as _},
    path::Path,
    sync::Arc,
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

fn workspace(root: &Path) -> WorkspaceService {
    WorkspaceService::new(
        &[root.to_path_buf()],
        PolicyEngine::new(
            Some(ExecutionPolicy {
                default: PolicyDecision::Allow,
                per_agent_tool: BTreeMap::new(),
                per_root: BTreeMap::new(),
            }),
            Arc::new(Approve),
        ),
    )
    .expect("benchmark workspace")
}

fn write_lines(path: &Path, bytes: usize) {
    let file = std::fs::File::create(path).expect("create benchmark fixture");
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    let line = b"alpha beta gamma delta epsilon zeta eta theta\n";
    let mut written = 0;
    while written < bytes {
        writer.write_all(line).expect("write benchmark fixture");
        written += line.len();
    }
    writer.flush().expect("flush benchmark fixture");
}

fn benchmark_read_range(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    let directory = tempfile::tempdir().expect("benchmark directory");
    let mut group = c.benchmark_group("fs_read_middle_range");
    for size in [1024 * 1024_usize, 10 * 1024 * 1024] {
        let path = directory.path().join(format!("read-{size}.txt"));
        write_lines(&path, size);
        let workspace = workspace(directory.path());
        let request = TextReadRequestV2 {
            path,
            range: TextReadRange::Byte {
                start: (size / 2) as u64,
                limit: 4096,
            },
            max_bytes: 4096,
            include_line_endings: true,
            expected_version: None,
            budget: TextReadBudget {
                timeout_ms: 30_000,
                max_bytes_read: 64 * 1024,
            },
        };
        group.throughput(Throughput::Bytes(4096));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, _| {
            bencher.iter(|| {
                let result = runtime
                    .block_on(workspace.read_text_v2(None, black_box(&request)))
                    .expect("benchmark read");
                black_box(result.content)
            });
        });
    }
    group.finish();
}

fn benchmark_search(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    let directory = tempfile::tempdir().expect("benchmark directory");
    for index in 0..1_000 {
        std::fs::write(
            directory.path().join(format!("search-{index:04}.txt")),
            b"alpha beta gamma delta\n",
        )
        .expect("write search fixture");
    }
    let workspace = workspace(directory.path());
    let request = FsSearchRequest {
        path: directory.path().to_path_buf(),
        query: "not-present".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: true,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 1,
        limit: 1,
        max_snippet_bytes: 128,
        budget: FsSearchBudget {
            timeout_ms: 30_000,
            max_files_scanned: 2_000,
            max_bytes_scanned: 16 * 1024 * 1024,
            max_output_bytes: 1024,
            max_file_bytes: 1024,
        },
    };
    c.bench_function("fs_search_1000_files_no_match", |bencher| {
        bencher.iter(|| {
            let context = OperationContext::new("criterion", "benchmark", "fs_search");
            let result = runtime
                .block_on(workspace.search_v2(&context, black_box(&request), None, None, |_| {}))
                .expect("benchmark search");
            black_box(result.0.files_scanned)
        });
    });
}

fn benchmark_atomic_replace(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    let directory = tempfile::tempdir().expect("benchmark directory");
    let path = directory.path().join("atomic.txt");
    std::fs::write(&path, b"old").expect("seed atomic fixture");
    let workspace = workspace(directory.path());
    let content = "x".repeat(1024 * 1024);
    c.bench_function("fs_atomic_replace_1mib", |bencher| {
        bencher.iter(|| {
            let context = OperationContext::new("criterion", "benchmark", "fs_write_text");
            let result = runtime
                .block_on(workspace.write_text_atomic(
                    &context,
                    &path,
                    black_box(&content),
                    AtomicWriteOptions {
                        overwrite: true,
                        ..AtomicWriteOptions::default()
                    },
                ))
                .expect("benchmark atomic replacement");
            black_box(result.bytes_written)
        });
    });
}

criterion_group!(
    benches,
    benchmark_read_range,
    benchmark_search,
    benchmark_atomic_replace
);
criterion_main!(benches);
