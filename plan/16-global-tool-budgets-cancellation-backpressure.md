# Plan 16 — Thêm budget, cancellation cooperative, backpressure và admission control cho mọi tool nặng

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy xây một framework dùng chung để giới hạn tài nguyên và hủy tác vụ dài: timeout, bytes read/written, files/entries scanned, output bytes, process runtime, open files, memory reservations và progress rate. Migrate các tool filesystem traversal/mutation, Git và artifact/blob sang framework này. Không commit.

## Ưu tiên

**P0 — cross-cutting reliability.** Wrapper hiện có `CancellationToken`, nhưng blocking loops, filesystem traversal và subprocess không nhất thiết quan sát token; giới hạn response không đồng nghĩa giới hạn công việc đã làm.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/types.rs:73-111` `OperationContext` chứa `CancellationToken`.
- `src/runtime_host/persistence.rs:14-69` dùng `tokio::select!` giữa `dispatch(...)` và `context.cancellation.cancelled()`; drop future chưa chắc dừng `spawn_blocking` hoặc child process đã chạy.
- `src/runtime_host/activity_control.rs:1-179` registry giữ context/stop reason và API có thể cancel token, nhưng worker nội bộ chưa nhận budget/checkpoint thống nhất.
- `crates/chatcmd-runtime/src/filesystem.rs` và `filesystem_search.rs` có blocking traversal/read loops không kiểm tra token.
- `filesystem_mutations.rs` có recursive copy/delete trong `spawn_blocking` không cooperative cancellation.
- `services.rs` dùng child process output không có timeout/cancel/kill-tree.
- `RuntimeConfig` tại `types.rs:169-199` có `max_concurrent_operations`, `max_replay_bytes`, nhưng chưa thấy per-tool budget/admission framework xuyên suốt.

## Mục tiêu

1. Một typed `ToolBudget`/`BudgetTracker` áp dụng nhất quán cho mọi operation nặng.
2. Default server-side không thể bị caller nâng vượt hard cap; caller chỉ được yêu cầu thấp hơn hoặc trong policy.
3. Mọi loop blocking/async có checkpoint cancellation/budget đủ thường xuyên.
4. `spawn_blocking` worker không tiếp tục chạy lâu sau khi outer future bị drop.
5. Subprocess timeout/cancel giết cả process tree và reap.
6. Bounded channels/semaphores tạo backpressure; không có queue/event buffer unbounded.
7. Admission control từ chối sớm khi hệ thống không đủ slot/disk/memory reservation, với lỗi retryable rõ.
8. Result/error thống nhất lý do kết thúc và usage đã tiêu thụ.

## Contract đề xuất

Tạo types nội bộ/shared:

```rust
ToolBudget {
    deadline: Option<Instant>,
    max_files: Option<u64>,
    max_entries: Option<u64>,
    max_bytes_read: Option<u64>,
    max_bytes_written: Option<u64>,
    max_output_bytes: u64,
    max_open_files: u32,
    max_progress_events: u32,
    memory_reservation_bytes: Option<u64>,
}

BudgetTracker {
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    counters: Atomic/owned counters,
}
```

API nên có methods:

```rust
checkpoint()?;
consume_files(n)?;
consume_entries(n)?;
consume_read_bytes(n)?;
consume_write_bytes(n)?;
reserve_output(n)?;
remaining_*();
finish_usage();
```

Errors typed:

- `operationCancelled`;
- `timeBudgetExceeded`;
- `fileBudgetExceeded`;
- `byteBudgetExceeded`;
- `outputBudgetExceeded`;
- `resourceBusy`/`admissionDenied`;
- `diskQuotaExceeded`.

Mỗi error có `retryable`, usage summary và phase. Không dùng message string làm logic.

## Cấu hình và precedence

Budget cuối là min của:

1. hard safety cap compiled/configured;
2. account/plan/policy cap nếu có;
3. task/execution mode cap;
4. tool default;
5. caller-requested cap.

Caller không được gửi `timeoutMs=0` để vô hạn nếu server hard cap hữu hạn. Có thể dùng `null` nghĩa default; semantics phải rõ.

Config phải validate ở startup, hiển thị diagnostics và có sensible defaults theo tool. Ví dụ search read-only có 15–60 giây, recursive copy có thể dài hơn nhưng có max bytes/files và lease/progress.

## Cooperative cancellation cho blocking work

`spawn_blocking` không tự dừng khi future bị drop. Mỗi blocking worker phải nhận clone của tracker/token và gọi checkpoint:

- mỗi N entries;
- mỗi chunk 64 KiB–1 MiB;
- trước/after syscall đắt;
- trước commit/publish phase.

Recursive helpers nên đổi sang iterative stack/iterator để checkpoint và tránh stack depth vấn đề. Worker phải return sớm, cleanup RAII và được join/reaped. Outer dispatch không nên trả final `stopped` trước khi worker đã đạt safe cancellation point nếu worker còn có thể mutate target; cần cancellation protocol hai phase:

1. request cancellation;
2. worker rollback/cleanup;
3. terminal result xác nhận state.

## Backpressure/concurrency

- Global weighted semaphore cho CPU/I/O operation; read nhỏ weight thấp, recursive mutation/index weight cao.
- Per-task/per-agent semaphore để một actor không chiếm hết host.
- Bounded channels cho search progress, watcher events, process output, artifact writes.
- Open-file semaphore riêng cho recursive operations.
- Disk writer quota/reservation cho blob/artifact/staging.
- Queue có max length/TTL; khi đầy trả retryable error, không queue vô hạn.
- Không giữ semaphore permit qua unrelated user approval wait nếu chưa bắt đầu resource-heavy work.

## Progress throttling

Tạo `ProgressLimiter`:

- max events/second và max total events;
- coalesce latest counters/current path;
- always allow final terminal update;
- không persist mỗi low-level progress event nếu UI chỉ cần latest state.

Progress event phải gồm counters và elapsed, không content lớn.

## Partial result semantics

Quyết định theo loại tool:

- Read/search/list/find: có thể trả partial success + cursor/truncation reason nếu contract plan 02 cho phép.
- Mutation trước commit: cancel/fail và target unchanged/rolled back.
- Mutation sau commit: report committed result dù client cancel race, kèm `cancellationArrivedAfterCommit` nếu cần.
- Copy/move/delete nhiều item: state machine plan 12 quyết định rollback/partial.
- Git/process: terminal status timedOut/cancelled với bounded preview/artifact.

Không trả generic `activity_stopped` nếu operation thực tế vẫn mutate ngầm.

## Các bước triển khai

1. Lập ma trận tool → default/hard budgets → partial semantics → cancellation checkpoints.
2. Tạo shared types/config/error/result usage.
3. Tạo admission controller/semaphores và RAII permits.
4. Tạo progress limiter/bounded sink.
5. Migrate `fs_search` và `fs_find` làm mẫu.
6. Migrate streaming read/write/edit.
7. Migrate recursive copy/move/delete và watcher/index.
8. Migrate bounded Git/process runner.
9. Sửa `call_persisted` để đợi worker cleanup/terminal state đúng thay vì chỉ drop dispatch future.
10. Expose budget fields trong MCP schema có defaults/hard max mô tả rõ.
11. Thêm diagnostics/metrics và docs.

## Edge cases bắt buộc

- Cancel trước start, lúc đang queue, giữa blocking read, giữa write staging, trước/after commit.
- Timeout và explicit cancel xảy ra đồng thời.
- Budget vượt chính xác ở boundary/chunk overshoot.
- Slow consumer/progress sink/database.
- Semaphore permit leak khi panic/error/cancel.
- Recursive operation triệu file và open file exhaustion.
- App shutdown trong operation.
- Caller yêu cầu cap cao hơn server hoặc malformed zero/overflow.
- System clock change; dùng monotonic `Instant` cho deadline.

## Test bắt buộc

- Unit test mỗi counter/boundary/precedence.
- Fake clock/deadline deterministic.
- Blocking worker nhận cancel và dừng trong latency bound.
- Outer call không return stopped trong khi mutation worker vẫn chạy.
- Semaphore fairness/per-task cap/permit release.
- Bounded channel với producer nhanh consumer chậm.
- Search partial cursor khi budget hết.
- Write/edit cancel target unchanged; post-commit cancel behavior.
- Recursive operation cleanup/rollback.
- Git child tree killed/reaped.
- Progress event count không vượt cap và final event luôn có.
- Fuzz/proptest arithmetic overflow/saturating counters.

## Benchmark bắt buộc

- Nhiều concurrent search/read/write tasks hơn slot.
- Event producer rất nhanh + slow subscriber.
- Measure cancellation latency, queue wait, memory, throughput và fairness.
- Không đặt threshold wall-clock quá cứng trên CI; assert hard invariants queue/counter/buffer.

## Tiêu chí nghiệm thu

- Mọi tool nặng nhận cùng budget/cancellation abstraction.
- Không có blocking traversal/mutation dài mà không checkpoint.
- Không có unbounded channel/queue trong pipeline liên quan.
- Subprocess được kill/reap khi timeout/cancel.
- Caller không nâng vượt hard caps.
- Result/error cho biết reason và resource usage.
- Stop UI phản ánh terminal state thật, không chỉ cancel outer future.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

## Kết quả AI phải trả về

- Tool budget matrix và defaults/hard caps.
- Framework types và admission/backpressure architecture.
- Danh sách tool đã migrate/chưa migrate.
- Cancellation semantics theo read/mutation/process.
- Test/benchmark cancellation/fairness/memory.
- Các breaking changes/schema migration.
