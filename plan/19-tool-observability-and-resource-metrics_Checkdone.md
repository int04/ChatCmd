# Plan 19 — Bổ sung observability và resource metrics cho tool runtime

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy xây observability thống nhất cho toàn bộ tool runtime: structured tracing spans, counters/histograms, per-operation resource usage, diagnostics endpoint và correlation task/turn/request/session. Tuyệt đối không log file content, Base64, command input nhạy cảm, token hoặc private conversation scope. Không commit.

## Ưu tiên

**P1 — vận hành và tối ưu.** Không thể xác nhận tool đã thật sự phù hợp project lớn nếu chỉ biết output bị truncate mà không đo bytes đọc, files scan, RAM/buffer, queue wait, cancellation latency và dữ liệu persist/realtime.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/persistence.rs:14-135` đã có request/tool/status correlation trong timeline nhưng lưu payload lớn và chưa phải resource telemetry.
- `src/runtime_host.rs:196-214` gọi `statistics::record_mcp_success`; module statistics hiện chủ yếu đếm usage nghiệp vụ, không phải performance/health telemetry.
- `crates/chatcmd-runtime/src/types.rs:73-111` `OperationContext` có request/task/turn/session fields phù hợp làm correlation.
- `src/runtime_host/activity_control.rs` biết active operation và stop reason nhưng chưa expose duration/resource phase.
- `crates/chatcmd-runtime/src/filesystem_search.rs` có `files_scanned/matches_found` progress cục bộ; các tool khác không dùng schema/counters chung.
- `crates/chatcmd-runtime/src/services.rs` chỉ trả exit/output/truncated; không có queue wait, process runtime, bytes streamed hay kill reason đầy đủ.

Plan này nên đọc kết quả plan 02 và 16 để dùng chung `ToolUsage`/`BudgetTracker`, không tạo bộ counter thứ hai lệch nghĩa.

## Mục tiêu

1. Mỗi tool call có một tracing span với correlation và phase, không chứa content nhạy cảm.
2. Có counters/histograms bounded-cardinality cho success/failure/cancel/timeout, elapsed, queue wait, files/bytes/output, truncation, artifact externalization và retry.
3. Result envelope có usage user-facing nhỏ; telemetry nội bộ chi tiết hơn nhưng không nhân đôi payload.
4. Diagnostics local cho biết tool nào chậm/tốn tài nguyên, active operations, queue depth, dropped/coalesced events, blob/artifact/journal usage.
5. Logging/error chain đúng một lần ở boundary phù hợp; không log cùng error nhiều tầng.
6. Cardinality được kiểm soát: không dùng raw path/task/request/error message làm metric label.
7. Có sampling/rate limit và retention; không làm telemetry thành bottleneck.
8. Test chứng minh redaction và metric correctness.

## Taxonomy đề xuất

### Span chính

`chatcmd.tool_call` fields:

- `tool.name` — enum/catalog key;
- `tool.class` — read/search/mutation/git/shell/agent;
- `request.id` — tracing field/log only, không metric label;
- `task.id`, `turn.id`, `session.id` — có thể hash/truncate trong logs, không metric labels;
- `agent.id` — không metric label;
- `execution.mode`;
- `budget.timeout_ms`, selected caps;
- `queue.wait_ms`;
- `result.status`, `error.code`, `truncation.reason`;
- `usage.elapsed_ms`, bytes/files/output;
- `artifact.created`, `rollback.state`, `atomic` nếu liên quan.

Dùng `#[tracing::instrument(skip(...))]` hoặc manual span; luôn skip arguments/content/readers/ciphers/tokens.

### Metrics

Tên ví dụ:

- `chatcmd_tool_calls_total{tool,status}`;
- `chatcmd_tool_duration_seconds{tool,status}`;
- `chatcmd_tool_queue_wait_seconds{class}`;
- `chatcmd_tool_bytes_read_total{tool}`;
- `chatcmd_tool_bytes_written_total{tool}`;
- `chatcmd_tool_files_scanned_total{tool}`;
- `chatcmd_tool_output_bytes_total{tool,destination=inline|artifact|timeline|realtime}`;
- `chatcmd_tool_truncations_total{tool,reason}`;
- `chatcmd_tool_cancellations_total{tool,phase}`;
- `chatcmd_tool_active{class}`;
- `chatcmd_tool_queue_depth{class}`;
- `chatcmd_artifact_bytes`, `chatcmd_blob_bytes`, `chatcmd_operation_journal_active`;
- `chatcmd_progress_events_total{emitted|coalesced|dropped}`;
- `chatcmd_subagent_lease_expired_total{reason}`.

Labels phải là finite enums. Raw path, extension hiếm, query, repository name, task ID, user email không được làm labels.

## `ToolUsage` dùng chung

Tạo hoặc dùng type từ plan 02/16:

```rust
ToolUsage {
    elapsed_ms: u64,
    queue_wait_ms: u64,
    files_scanned: Option<u64>,
    entries_scanned: Option<u64>,
    bytes_read: Option<u64>,
    bytes_written: Option<u64>,
    output_bytes: u64,
    progress_events: u32,
    retries: u32,
}
```

Counters phải update tại cùng điểm budget tracker consume để không drift. Finish method trả snapshot cho result và telemetry. Dùng saturating arithmetic và type conversion an toàn.

## Error/log policy

- Runtime library tạo typed error + context, không log ở mọi tầng.
- Boundary `call_persisted` hoặc top-level worker ghi một structured event terminal với full error chain nhưng sanitized.
- User-facing error message actionable nhưng không chứa content/path ngoài quyền.
- Known expected errors (not found, version conflict, cancelled, budget exceeded) log mức debug/info; unexpected I/O/internal mức warn/error.
- Redact keys/patterns: token, authorization, secret, password, key, base64, content, environment values, conversation scope raw.
- Không log command string ghép đầy đủ nếu có argument nhạy cảm; log executable/tool + argument count/safe flags.

## Diagnostics endpoint/UI

Local authenticated diagnostics trả bounded snapshot:

```json
{
  "activeOperations": [...bounded summaries...],
  "queues": { "ioDepth": 0, "cpuDepth": 0 },
  "lastWindow": {
    "toolCalls": 100,
    "timeouts": 1,
    "cancellations": 2,
    "bytesRead": 123456,
    "artifactBytes": 98765
  },
  "limits": { "...": "effective values" }
}
```

Không trả raw arguments/content. Active operations có tool, elapsed, phase, counters và hashed/authorized path summary nếu cần UI. Endpoint phải có pagination/cap nếu history.

Nếu không thêm Prometheus/OpenTelemetry dependency, có thể dùng internal atomic registry + tracing trước. Ghi ADR về export strategy. Không cần ép remote telemetry; local-only mặc định bảo vệ privacy.

## Phase instrumentation

Các operation dài nên set phase typed:

- queued, authorizing, resolvingPath, scanning, reading, staging, verifying, syncing, committing, rollingBack, cleaningUp;
- processStarting, processRunning, processStopping;
- artifactWriting;
- waitingApproval;
- waitingSubagent.

Phase giúp stop UI/diagnostics hiểu tool đang làm gì. Transition không cần persist mỗi lần nếu quá nhiều; latest state trong activity registry, terminal summary trong timeline.

## Các bước triển khai

1. Lập data classification/redaction policy và metric label review.
2. Tạo `ToolTelemetry` facade dùng `tracing` và metrics backend/internal registry.
3. Kết nối với budget/usage tracker; tránh duplicate counters.
4. Instrument top-level call lifecycle, authorization, queue/admission và terminal status.
5. Instrument read/search/mutation/Git/blob/artifact/subagent phase/counters.
6. Mở rộng activity registry với latest phase/usage snapshot có lock contention thấp.
7. Thêm diagnostics API/UI nếu phù hợp; giữ payload bounded.
8. Thêm log filtering/config và sampling/rate limit.
9. Thêm redaction test/fuzz/property tests.
10. Cập nhật docs vận hành và giải thích local privacy.

## Edge cases bắt buộc

- Tool argument chứa secret marker nested sâu.
- Error từ OS chứa path nhạy cảm hoặc content snippet.
- Metric counter overflow.
- Telemetry backend lỗi/chậm; tool không được fail vì telemetry.
- Nhiều operation đồng thời/cardinality explosion.
- Cancellation/timeout race, rollback sau terminal request.
- App shutdown trước flush.
- Clock wall-time thay đổi; duration dùng monotonic clock.
- Event sampling không bỏ terminal/error event cần thiết.

## Test bắt buộc

- Mỗi success/failure/cancel/timeout tăng đúng counter một lần.
- Duration/queue wait/bytes/files match fake operation.
- No duplicate terminal metric khi retry/idempotent request.
- Metric labels chỉ thuộc allowlist finite.
- Capture logs/diagnostics không chứa marker content/Base64/token/password/scope.
- Telemetry failure không làm tool call fail.
- Active operation phase/counters cập nhật và cleanup.
- Progress coalesced/dropped counters đúng.
- Subagent lease/blob/artifact/journal metrics tích hợp.
- Snapshot diagnostics bounded với hàng nghìn active/history records giả lập.

## Benchmark bắt buộc

- So sánh throughput/latency tool hot path khi telemetry off/on.
- 10.000+ events/calls giả lập; đo allocation/lock contention.
- Telemetry overhead mục tiêu nhỏ và được báo số liệu, không đặt claim không đo.

## Tiêu chí nghiệm thu

- Có một telemetry facade, không rải counter/log ad hoc.
- Span/counters dùng typed finite labels và correlation đúng.
- Resource usage lấy từ cùng tracker enforce budget.
- Logs/metrics/diagnostics không chứa content/secrets.
- Diagnostics cho biết operation phase, queue, limits và aggregate usage.
- Telemetry lỗi không ảnh hưởng correctness tool.
- Có benchmark overhead và redaction regression tests.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Chạy frontend test/build nếu thêm diagnostics UI.

## Kết quả AI phải trả về

- Taxonomy spans/metrics/labels cuối.
- Redaction/data classification policy.
- File/module/API/UI đã đổi.
- Cách tích hợp ToolUsage/BudgetTracker.
- Test privacy/correctness và benchmark overhead.
- Cách bật/tắt/export telemetry.

## Kết quả rà soát hoàn tất

Plan 19 đã được rà soát lại và không còn blocker cần xử lý.

- `cargo fmt --check` — **PASS**.
- `cargo check --workspace` — **PASS**.
- `cargo test --workspace` — **PASS**; toàn bộ test workspace thành công, các benchmark/test được đánh dấu `ignored` vẫn giữ nguyên theo thiết kế.
- `cargo clippy --workspace --all-targets -- -D warnings` — **PASS** sau khi xử lý toàn bộ lint còn tồn tại trên workspace với Rust/Clippy hiện tại.
- `cargo bench -p chatcmd-runtime --bench tool_telemetry` — **PASS** với batch 10.000 calls:
  - telemetry off: median khoảng **1,2281 ms / 10.000 calls** (~8,14 Melem/s);
  - telemetry on: median khoảng **7,5783 ms / 10.000 calls** (~1,32 Melem/s);
  - telemetry on, 4 luồng contention: median khoảng **19,191 ms / 10.000 calls** (~521 Kelem/s).

Các regression test telemetry trong `chatcmd-runtime` xác nhận redaction marker nhạy cảm, counter saturation/overflow safety, terminal status/counter correctness, diagnostics bounded, progress/subagent/resource metrics và failure isolation. `ToolUsage` tiếp tục dùng chung với budget/resource tracking; không tạo hệ counter enforcement thứ hai.

Các lint mới của Clippy liên quan `RuntimeError` lớn được xử lý bằng allow có phạm vi crate/test với chú thích rõ ràng, vì boxing `RuntimeError` sẽ là thay đổi API xuyên tầng không thuộc phạm vi Plan 19. Các lint hành vi/style còn lại được sửa trực tiếp hoặc allow cục bộ ở các API ổn định có nhiều tham số.
