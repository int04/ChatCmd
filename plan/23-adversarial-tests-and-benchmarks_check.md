# Plan 23 — Xây adversarial test, crash/fault harness và benchmark cho project/file lớn

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy xây một bộ test/benchmark có thể chứng minh các filesystem, Git, shell, persistence và orchestration tools hoạt động an toàn dưới tải lớn và tình huống đối kháng. Không chỉ thêm unit test happy-path; cần fixture generators, fault injection, concurrency barriers, packaged-binary smoke tests và CI tiers. Không sửa thuật toán production ngoài các seam tối thiểu phục vụ testability. Không commit.

## Ưu tiên

**P2 như một plan tổng hợp, nhưng từng P0/P1 feature không được coi hoàn tất nếu thiếu test trực tiếp.** Plan này gom infrastructure và coverage xuyên tool, không thay thế test trong từng plan.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/tests/direct_runtime.rs` trước audit có khoảng 9 integration tests, gồm line-range/replace cơ bản, traversal denial, search ignore và shell lifecycle/backpressure.
- `crates/chatcmd-runtime` unit tests trước audit khoảng 10; phần lớn filesystem mutation/search lớn chưa có adversarial coverage.
- `crates/chatcmd-mcp` tests trước audit khoảng 31, chủ yếu catalog/schema/security/orchestration; catalog test in-process chưa bắt được binary/connector drift.
- `filesystem_mutations.rs`, `filesystem_search.rs`, `filesystem.rs` hiện chưa có test file hàng trăm MB/GB, crash atomicity, concurrent writers, permission matrices, symlink swap hoặc rollback recursive đầy đủ.
- `turn_file_changes.rs` chỉ có test nhận diện tempfile names ở phần đã đọc.
- `subagents.rs`/finalization watchdog cần regression cho running child stale.
- Audit trước đã chạy xanh `cargo test -p chatcmd-runtime` và `cargo test -p chatcmd-mcp`; đây là baseline correctness nhỏ, chưa phải bằng chứng scalability.

AI phải inventory lại toàn bộ `#[test]`, integration tests, CI workflows và benchmark config trước khi thêm.

## Mục tiêu

1. Có test pyramid rõ: deterministic unit, integration filesystem thật, subprocess/crash harness, packaged binary/transport, stress/perf benchmark.
2. Fixture lớn được sinh streaming/sparse để CI không commit giant files vào Git.
3. Đo và assert resource invariants: bytes thực đọc, max buffer/channel, output/persistence size, cancellation latency, cleanup—not chỉ wall time.
4. Có fault injection có tên tại các commit phase để tái hiện partial write/copy/move/delete.
5. Có race tests deterministic bằng barriers/hooks, không dựa vào sleep may rủi.
6. Cross-platform path/permissions/symlink/junction/process-tree behavior có test theo OS.
7. CI chia tier để PR vẫn nhanh, nightly/manual chạy stress lớn.
8. Benchmark tạo report so sánh baseline/threshold có tolerance, không flaky vì máy CI.
9. Không để test làm mất dữ liệu ngoài temp workspace hoặc dùng path rộng.

## Cấu trúc đề xuất

```text
crates/chatcmd-runtime/tests/
  filesystem_read_large.rs
  filesystem_edit_concurrency.rs
  filesystem_mutation_recovery.rs
  filesystem_traversal_large.rs
  git_large_output.rs
  shell_backpressure.rs
  support/
    fixtures.rs
    fault_injection.rs
    resource_probe.rs
    process_helper.rs

crates/chatcmd-mcp/tests/
  packaged_catalog.rs
  large_payload_contract.rs
  transport_identity.rs

benches/
  fs_read_range.rs
  fs_search.rs
  fs_apply_edits.rs
  recursive_mutations.rs
  git_output.rs
  terminal_coalescing.rs
```

Có thể bố trí khác theo workspace nhưng helper phải tái sử dụng, file dưới 500 dòng và không copy-paste fixture logic.

## Test fixture infrastructure

### Large file generator

API helper tạo file mà không giữ full data trong RAM:

- deterministic repeating/random seed;
- configurable bytes, line length, newline, BOM, UTF-8/binary;
- sparse file mode khi chỉ test seek/size;
- streaming checksum/known markers ở đầu/giữa/cuối;
- mutation helper append/truncate/atomic replace.

Sizes tier:

- unit/smoke: 1–10 MiB;
- PR integration: 50–100 MiB tùy thời gian;
- nightly/manual: 1 GiB+;
- không commit fixture binary.

### Large tree generator

- configurable depth/fanout/files;
- 10k PR, 100k–1m nightly;
- `.git`, `target`, `node_modules`, ignored/negated patterns;
- symlink loops/broken links/outside-root targets;
- permission-denied subtree;
- non-UTF-8 names Unix, case collisions theo platform;
- cleanup RAII, retry Windows locks bounded.

### Resource probe

Ưu tiên deterministic counters instrumented từ production abstractions:

- bytes read/written;
- entries/files/metadata calls;
- max buffer/channel depth;
- open handles;
- output/timeline/realtime/artifact bytes;
- worker/process count;
- temp/journal leftovers.

Peak RSS có thể đo supplementary và report, nhưng không dùng threshold tuyệt đối quá chặt trên shared CI. Assert invariant mạnh như `maxBufferedBytes <= configuredCap + boundedOverhead`.

## Fault injection framework

Tạo seam chỉ bật test/feature nội bộ, typed fault points:

```rust
BeforeTempCreate
AfterTempCreate
AfterBytesWritten(u64)
BeforeFileSync
AfterFileSync
BeforeVersionRecheck
BeforeAtomicReplace
AfterAtomicReplace
BeforeDirectorySync
AfterNFilesCopied(u64)
BeforeDestinationPublish
AfterDestinationPublish
BeforeSourceDelete
DuringRollback
BeforeJournalCommit
AfterJournalCommit
```

Fault injector có thể:

- return I/O error cụ thể;
- block trên barrier để concurrent writer hành động;
- simulate cancellation;
- abort child process cho crash harness.

Không để production caller điều khiển fault injection. Compile/test-only hoặc protected dependency injection.

## Ma trận test bắt buộc

### `fs_read_text`/range

- UTF-8 10/100 MiB/1 GiB, range đầu/giữa/cuối.
- CRLF/LF/CR/mixed, BOM, invalid UTF-8 trước/trong/sau range.
- line cực dài, UTF-8 chunk boundary, file đổi giữa read.
- byte/line budgets, cursor stale, cancellation.
- assert bytes buffered/read theo contract.

### `fs_stat`/version

- same size content change, mtime collision, atomic replace identity.
- forged/cross-path token, old version schema.
- strong hash streaming/cancel.

### `fs_apply_edits`/write

- insert/delete/replace/multiple ranges/overlap.
- concurrent writers tại barriers.
- failure/crash ở every atomic phase; target old hoặc new complete.
- mode/readonly/executable/BOM/newline preservation.
- disk full/short write/rename/sync failures.
- temp cleanup/startup recovery.

### Copy/move/delete

- 100k files, cross-device move, destination conflict.
- failure sau N files, verify mismatch, source mutation.
- cancellation/restart/journal recovery/rollback.
- symlink swap, source/destination nesting, quarantine/restore.

### Search/find/list/index

- early stop thật, cursor continuation, ignore parity.
- time/file/byte/entry budget.
- watcher/index stale/overflow/corruption/fallback.
- non-UTF-8 path, permissions, tree mutation giữa pages.

### Git

- stdout/stderr lớn đồng thời, hanging hook/helper.
- timeout/cancel/kill process tree.
- status 100k entries, binary/rename/non-UTF-8.
- artifact quota và bounded inline output.

### Shell

- 1-byte event storm, large sustained output, slow/no consumer.
- UTF-8/ANSI/CR split, replay eviction/gap.
- giant input rejected, sensitive input redacted.
- close/exit/write/read/stop races; orphan process cleanup.

### Persistence/realtime

- secret marker/full content không xuất hiện trong SQLite/log/WebSocket/statistics.
- event/realtime cap, artifact refs, old event migration.
- slow DB/subscriber, queue overflow/degradation.
- database growth proportional summary, không content.

### MCP/catalog/identity

- packaged binary mở transport thật và advertise exact manifest/hash.
- connector/cache refresh on catalog change.
- spoofed identity, JSON-RPC batch, body limits.
- blob upload/reconnect/cross-owner/quota/hash.

### Sub-agent

- running worker disappears, lease expiry, parent finalizes.
- heartbeat/max runtime, completion-timeout race, stale attempt result.
- restart and extension fallback behavior.

## Crash test harness

Một process helper thực hiện mutation tới fault point rồi `abort`/kill. Parent process:

1. tạo baseline file/tree;
2. spawn helper với isolated temp workspace/db;
3. đợi marker/fault phase qua IPC/file descriptor;
4. kill/abort;
5. restart/recovery component;
6. kiểm tra target, temp, journal, source/destination và DB event state.

Không dựa vào Rust unwinding cho crash test vì crash thật không chạy `Drop`. Mỗi test dùng unique temp root và cleanup best-effort.

## Concurrency/race harness

- Dùng `Barrier`, channels/oneshot hoặc injected callbacks để điều khiển exact interleaving.
- Hai writer cùng expectedVersion: chỉ một commit, writer kia conflict.
- Reader/file replace giữa stat và read.
- Symlink swap giữa resolve và syscall.
- Completion vs watchdog timeout.
- Artifact consume vs GC/abort.
- Approval budget concurrent consume.

Có thể dùng `loom` cho state machines/channels nhỏ, nhưng filesystem/process races cần integration tests thật.

## Benchmark strategy

- Dùng Criterion nếu phù hợp; benchmark I/O phải có fixture setup ngoài measured section, `black_box` và cleanup.
- Lưu machine metadata và config trong report.
- So sánh medians/distributions, throughput MB/s, files/s, time-to-first-result, memory/buffer, events/DB writes.
- PR chỉ chạy smoke/perf invariant; nightly chạy sizes lớn và regression report.
- Threshold:
  - hard correctness/resource caps tuyệt đối;
  - performance regression tương đối so baseline/recent median với tolerance rộng;
  - không fail vì 5–10% noise nếu không có ý nghĩa.

## CI tiers đề xuất

### Tier 0 — mỗi PR, nhanh

- Unit/property/schema/security.
- Small integration fixtures.
- `cargo fmt --check`, `cargo check --workspace`, `cargo test --workspace`.

### Tier 1 — PR hoặc merge, trung bình

- 50–100 MiB files, 10k–50k paths.
- cancellation, fault injection non-crash, packaged MCP smoke.
- Windows/macOS/Linux platform matrix theo khả năng.

### Tier 2 — nightly/manual

- 1 GiB files, 100k–1m paths, process crash, cross-device, event storm, concurrency soak.
- benchmark report/artifact.

- Tests cần privilege/symlink/permission đặc biệt phải skip với lý do structured, không silently pass.

## Các bước triển khai

1. Inventory coverage hiện tại và lập traceability matrix plan 01–22 → tests.
2. Tạo shared fixture/resource/fault/process helpers.
3. Thêm deterministic unit/integration tests cho P0 trước.
4. Thêm packaged binary/transport catalog test.
5. Thêm crash helper và mutation recovery matrix.
6. Thêm large tree/file/search/Git/shell benchmarks.
7. Tích hợp CI tiers, timeouts, artifact reports và cleanup.
8. Thêm platform-specific jobs/feature gates.
9. Ghi baseline metrics hiện tại và expected invariants.
10. Cập nhật developer docs: chạy local, chọn size/tier, debug failure.

## Chống flaky test

- Không dùng fixed sleep để đồng bộ race; dùng barriers/events.
- Dùng monotonic time/fake clock cho lease/budget khi có thể.
- Unique temp dirs/ports/database per test.
- Bounded timeout cho mọi child/process/test.
- Kill/reap child trong cleanup guard.
- Log seed/config/phase khi fail.
- Retry chỉ cho platform cleanup/sharing violation có lý do, không retry assertion logic.
- Serial hóa test chỉ khi thực sự dùng global resource; ưu tiên dependency injection.

## Tiêu chí nghiệm thu

- Có traceability matrix cho mọi rủi ro lớn đã audit.
- Có deterministic fault/race harness, không chỉ happy-path tests.
- Có packaged binary/real transport catalog test.
- Có resource invariants chứng minh bounded memory/I/O/output/persistence.
- Có crash tests chứng minh atomic/rollback/recovery state.
- Có CI tiers và benchmark reports không làm PR thường quá chậm.
- Test chạy được trên macOS hiện tại và có coverage/compile path Windows/Linux phù hợp.
- Không có giant fixture commit vào repository.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Sau đó chạy các command tier mới do plan tạo, ví dụ:

```bash
cargo test --workspace --features adversarial-tests
cargo bench --workspace
```

Tên feature/command thực tế phải được tài liệu hóa, không bắt buộc dùng đúng ví dụ trên.

## Kết quả AI phải trả về

- Coverage/traceability matrix trước và sau.
- Test infrastructure/file đã thêm.
- Fault/crash/race strategy.
- CI tier/workflow changes.
- Benchmark cases, baseline và resource invariants.
- Command chạy local/CI và kết quả đầy đủ.
- Test nào còn platform/manual-only và lý do.

## CẦN KIỂM TRA LẠI — Các hạng mục chưa được xác minh tự động

Plan 23 đã thêm và chạy thành công bộ dữ liệu kiểm thử dạng luồng/thưa, phép dò tài nguyên, cổng lỗi
có tên, tranh chấp trình ghi có phiên bản mang tính tất định, kết thúc/thu hồi tiến trình con sau giai
đoạn chuẩn bị, tìm kiếm cây sinh tự động có giới hạn, chuyển đầu ra Git lớn sang tệp, phạm vi kiểm
thử vận chuyển đóng gói hiện có, phép đo hiệu năng hệ thống tệp bằng Criterion và các tầng CI. Tuy
nhiên, plan phải giữ hậu tố `_check` vì các hạng mục sau chưa thể chạy đầy đủ trên máy Windows hiện
tại:

- Chưa chạy dữ liệu đặc 100 MiB/1 GiB, cây 100.000–1.000.000 tệp và kiểm thử ngâm nhiều giờ. Cần
  chạy trong thư mục làm việc dùng một lần bằng Tầng 2 có đủ dung lượng đĩa/thời gian rồi lưu hạt giống,
  cấu hình và báo cáo.
- Cổng lỗi có tên mới là hàm hỗ trợ kiểm thử; chưa nối điểm can thiệp chỉ dành cho kiểm thử vào mọi
  giai đoạn xác nhận thay đổi trong môi trường thực tế. Cần chèn lỗi và chứng minh trạng thái cũ
  hoặc mới cho các lỗi đầy đĩa, ghi ngắn, đồng bộ, đổi tên, nhật ký, xuất bản, xóa nguồn và hoàn tác.
- Chưa có điểm gắn kết thứ hai để kiểm thử di chuyển qua thiết bị; chưa có ma trận quyền nâng cao,
  đường dẫn Unix không phải UTF-8, liên kết tượng trưng macOS và ma trận điểm nối/hoán đổi liên kết
  tượng trưng trên Windows.
- Chưa kiểm thử lỗi hook Git/trình hỗ trợ thông tin xác thực bị treo và việc dọn dẹp toàn bộ cây tiến
  trình hậu duệ.
- Chưa đo RSS cực đại/số tài nguyên xử lý đang mở trên máy chạy được kiểm soát, tình trạng chậm của cơ sở dữ
  liệu/bên đăng ký, tràn WebSocket, 100k mục trạng thái Git, bão dữ liệu shell mỗi lần một byte và
  kiểm thử ngâm đồng thời chạy lâu.
- Luồng công việc mới chỉ được kiểm tra cú pháp/đánh giá và đường biên dịch cục bộ; các tác vụ
  Ubuntu/macOS/Windows do GitHub lưu trữ chưa được thực thi từ môi trường cục bộ này.

Các lệnh đã chạy cục bộ:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p chatcmd-runtime --test adversarial_filesystem --no-fail-fast
cargo bench -p chatcmd-runtime --bench filesystem_workloads -- --sample-size 10 --measurement-time 1 --warm-up-time 1
cargo bench -p chatcmd-runtime --bench tool_telemetry -- --sample-size 10 --measurement-time 1 --warm-up-time 1
```

Kết quả tại thời điểm ghi: bước định dạng/kiểm tra thành công; `cargo test --workspace --no-fail-fast`
thành công (các bộ lần lượt có 102, 2, 39, 2, 82, 8, 3, 12, 7, 7, 2, 8, 1 và 9 kiểm thử thành công;
6 kiểm thử thủ công mang thuộc tính `ignored`); Criterion hoàn tất 7 trường hợp. Lệnh
`cargo clippy --workspace --all-targets -- -D warnings` thất bại với các lỗi đã tồn tại ngoài tệp
Plan 23 và không được sửa vì phạm vi plan này chỉ thêm kiểm thử:

- `filesystem_find.rs:213`: `clippy::collapsible_if`;
- `filesystem_read.rs:115`: `clippy::too_many_arguments`;
- `filesystem_read.rs:540`: `clippy::redundant_guards`;
- `filesystem.rs:338`: `clippy::too_many_arguments`;
- `process_runner.rs:77`: `clippy::manual_clamp`;
- `filesystem/file_version.rs:524`: `clippy::items_after_test_module`.

Người dùng cần quyết định tái cấu trúc hoặc cho phép các cảnh báo lint đối với những API trong môi
trường thực tế ở trên, rồi chạy lại đúng lệnh Clippy. Do bước kiểm tra này chưa thành công, tệp giữ
hậu tố `_check.md` theo quy tắc bàn giao.
