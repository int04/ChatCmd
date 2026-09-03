use chatcmd_runtime::{OperationContext, ToolStatus, ToolTelemetryRegistry, ToolUsage};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

const CALLS_PER_BATCH: u64 = 10_000;

fn run_batch(enabled: bool) {
    let registry = ToolTelemetryRegistry::new(enabled);
    for index in 0..CALLS_PER_BATCH {
        let context = OperationContext::new(index.to_string(), "benchmark", "fs_search");
        let call = registry.start(black_box(&context), black_box("fs_search"));
        call.finish(
            ToolStatus::Success,
            black_box(ToolUsage {
                files_scanned: Some(3),
                bytes_read: Some(4_096),
                output_bytes: 512,
                ..ToolUsage::default()
            }),
            None,
            false,
        );
    }
    black_box(registry.snapshot());
}

fn run_contended_batch() {
    let registry = ToolTelemetryRegistry::new(true);
    std::thread::scope(|scope| {
        for worker in 0..4_u64 {
            let registry = registry.clone();
            scope.spawn(move || {
                for index in 0..(CALLS_PER_BATCH / 4) {
                    let request = worker
                        .saturating_mul(CALLS_PER_BATCH)
                        .saturating_add(index)
                        .to_string();
                    let context = OperationContext::new(request, "benchmark", "fs_search");
                    registry.start(&context, "fs_search").finish(
                        ToolStatus::Success,
                        ToolUsage::default(),
                        None,
                        false,
                    );
                }
            });
        }
    });
    black_box(registry.snapshot());
}

fn benchmark_tool_telemetry(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_telemetry_10000_calls");
    group.throughput(Throughput::Elements(CALLS_PER_BATCH));
    for enabled in [false, true] {
        group.bench_with_input(
            BenchmarkId::from_parameter(if enabled { "on" } else { "off" }),
            &enabled,
            |bencher, enabled| bencher.iter(|| run_batch(*enabled)),
        );
    }
    group.bench_function("on_4_threads", |bencher| bencher.iter(run_contended_batch));
    group.finish();
}

criterion_group!(benches, benchmark_tool_telemetry);
criterion_main!(benches);
