# Plan 15 — Streaming, timeout, cancellation và artifact cho Git tools

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy sửa `git_status`, `git_diff`, `git_log`, `git_branch`, `git_show` và các bước subprocess của `git_commit` để không gom stdout/stderr vô hạn bằng `Command::output()`. Tạo process runner streaming có hard limits, timeout, cancellation, kill process tree và artifact/reference cho output lớn. Không commit.

## Ưu tiên

**P1 — memory và khả năng dừng.** `git diff`/`git show` trên monorepo hoặc binary/generated changes có thể trả output rất lớn; cap hiện tại chỉ được áp dụng sau khi toàn output đã nằm trong RAM.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/services.rs:8-24` `GitService` chỉ giữ `max_characters`.
- `services.rs:27-110` các Git method cuối cùng gọi `run`.
- `services.rs:111-122` `run` dùng `Command::output().await`, sau đó `bound_output`.
- `services.rs:83-105` `git add` trong commit cũng dùng `command.output()`.
- `services.rs:225-247` `bound_output` mới convert/truncate stdout/stderr sau process kết thúc; raw `Vec<u8>` đầy đủ đã được cấp phát.
- `src/main.rs:182` khởi tạo `GitService::new(workspace.clone(), 200_000)`; giới hạn này không phải limit RAM/subprocess output thực.
- `src/runtime_host/inputs.rs:240-275` Git inputs chưa có timeout/output/artifact/cursor/budget.

Plan này nên dùng result envelope plan 02, artifact plan 13, global budget plan 16 và observability plan 19.

## Mục tiêu

1. Không dùng `Command::output()` cho Git command có output không biết trước kích thước.
2. Đọc stdout/stderr đồng thời qua pipe với buffer/channel bounded để tránh deadlock.
3. Có `timeoutMs`, `maxOutputBytes`, `maxStderrBytes`, `maxRuntimeMs` và cancellation thực sự.
4. Khi output vượt inline cap:
   - tiếp tục stream vào artifact trong tổng disk quota nếu caller yêu cầu; hoặc
   - ngừng/kill process theo mode;
   - trả preview, size, truncation reason và artifact ref.
5. Kill toàn process tree/hook/child process khi timeout/cancel.
6. Output parsing được cải thiện cho status/log khi có thể, thay vì trả blob string khó dùng.
7. Không cho argument injection; tiếp tục dùng `Command.args` và `--` cho path.
8. Git hooks/interactions không được treo chờ stdin/credential prompt.

## Contract đề xuất

Tạo options dùng chung:

```json
{
  "cwd": ".",
  "staged": false,
  "stat": false,
  "path": null,
  "outputMode": "inlineOrArtifact",
  "maxOutputBytes": 524288,
  "timeoutMs": 30000,
  "artifactMaxBytes": 268435456,
  "killOnLimit": false
}
```

Result:

```json
{
  "exitCode": 0,
  "stdout": "bounded preview",
  "stderr": "bounded preview",
  "truncated": true,
  "truncationReason": "contentExternalized",
  "stdoutBytes": 12345678,
  "stderrBytes": 0,
  "artifactRef": "artifact:...",
  "elapsedMs": 1500,
  "timedOut": false,
  "cancelled": false
}
```

Với `git_status_v2`, nên trả structured entries từ porcelain v2 `-z`, cùng branch metadata và raw artifact optional:

```json
{
  "branch": { "head": "main", "upstream": "origin/main", "ahead": 2, "behind": 0 },
  "entries": [{ "path": "src/a.rs", "indexStatus": "M", "worktreeStatus": " " }],
  "nextCursor": null,
  "hasMore": false
}
```

## Process runner chung

Tạo `BoundedProcessRunner` dùng cho Git và có thể tái sử dụng cho process/shell không PTY:

- `stdin(Stdio::null())` hoặc explicit input source bounded.
- `stdout/stderr(Stdio::piped())`.
- Hai reader task đọc đồng thời theo chunk.
- Inline preview buffer bounded.
- Optional artifact writer streaming bounded/quota-aware.
- Counter atomic/central aggregator.
- `tokio::select!` trên child wait, timeout, cancellation và reader errors.
- Khi stop: gửi graceful signal nếu phù hợp, sau grace period kill tree.
- Join/reap child và reader tasks trước return.
- Không giữ mutex qua `.await`.

Channel giữa reader và sink phải bounded; hoặc mỗi reader stream trực tiếp vào sink synchronized có minimal critical section. Slow artifact disk phải tạo backpressure, không queue unbounded.

## Git command behavior

### Environment chống prompt

Set an toàn, ví dụ:

- `GIT_TERMINAL_PROMPT=0`;
- credential UI/pager disabled;
- `GIT_PAGER=cat`, `PAGER=cat`;
- color disabled;
- optional hooks policy cho `git commit` phải giữ behavior hiện tại nhưng timeout/cancel được.

Không vô hiệu hóa hooks âm thầm nếu product muốn chạy hooks; thay vào đó report phase/hook timeout rõ.

### `git_status`

- Dùng `git status --porcelain=v2 -z --branch`.
- Parse bytes, hỗ trợ rename/copy và non-UTF-8 path bằng representation an toàn (`pathBytesBase64` khi cần).
- Có item/page cap và artifact/raw fallback.

### `git_diff`/`git_show`

- Có `--no-ext-diff`, `--no-color`, pager off.
- Cho options summary-first: `stat`, `nameOnly`, `numstat`, patch.
- Patch lớn externalize; caller có thể đọc artifact theo range.
- Path/revision validation tiếp tục chặt.

### `git_log`/`git_branch`

- Dùng machine-readable separators/formats thay vì parse decorated human text khi structured result.
- Count cap và cursor theo commit hash/ref khi hợp lý.

### `git_commit`

- Stage và commit đều chạy qua process runner.
- Phân biệt timeout/cancel ở `git add`, hook, commit.
- Không báo commit success nếu process bị kill hoặc output sink lỗi.
- Result có commit hash khi thành công; kiểm tra bằng Git command bounded.

## Các bước triển khai

1. Tách Git service khỏi `services.rs` nếu cần giữ file dưới 500 dòng.
2. Tạo bounded process runner, typed limits/outcome/error.
3. Thêm process-tree termination abstraction Unix/Windows.
4. Migrate một read-only command (`git_diff`) và test output lớn.
5. Migrate status/log/branch/show và structured parsers.
6. Migrate stage/commit flow.
7. Tích hợp artifact store/result envelope/persistence projection.
8. Thêm MCP schema/options với default an toàn; giữ adapter fields cũ.
9. Thêm progress milestone theo thời gian/bytes, có throttle.
10. Cập nhật docs và server instructions: dùng stat/name-only trước patch lớn.

## Edge cases bắt buộc

- Diff hàng trăm MB; stderr đồng thời lớn.
- Process viết stderr đầy trong khi stdout sink chậm.
- Hanging hook/credential helper/submodule/ext-diff.
- Timeout/cancel race với process exit.
- Child/grandchild process còn sống sau kill.
- Repository path có spaces/non-UTF-8.
- Rename/copy/binary diff.
- No repository, corrupt repo, lock file, permission denied.
- Artifact quota đầy giữa output.
- Caller chỉ cần preview, caller cần full artifact.

## Test bắt buộc

- Fake child/process helper tạo stdout/stderr lớn đồng thời; memory bounded và không deadlock.
- Output limit với `killOnLimit=true/false`.
- Timeout và cancellation kill/reap process tree.
- Slow artifact sink tạo bounded backpressure.
- Git status porcelain parser cho modified/untracked/rename/non-UTF-8.
- Diff/show lớn trả preview + artifact đúng hash/size.
- Hanging pre-commit hook bị timeout/cancel.
- Argument/revision/path injection regression tests.
- Timeline không chứa full Git output sau plan 13.
- Compatibility output cũ hoặc versioned migration test.

## Benchmark bắt buộc

- Diff 10 MB/100 MB/1 GB generated fixture.
- Status repo 100.000 changed/untracked files.
- Đo peak memory, time-to-first-byte/preview, artifact throughput, cancel latency và process cleanup.

## Tiêu chí nghiệm thu

- Không còn `Command::output()` trong Git path có output lớn.
- Stdout/stderr được drain đồng thời với hard caps/backpressure.
- Timeout/cancel kill tree và process được reap.
- Output lớn có artifact/ref hoặc truncation reason rõ.
- Status/log structured đủ cho LLM, raw output vẫn optional.
- Git arguments vẫn không đi qua shell interpolation.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

## Kết quả AI phải trả về

- Process runner architecture.
- Git contracts/commands cuối.
- File đã đổi.
- Kill/timeout/artifact behavior theo OS.
- Parser tests và security checks.
- Benchmark memory/output/cancel số liệu.

## Kết quả hoàn tất 2026-09-04

Plan 15 đã hoàn tất và được xác minh lại trực tiếp trên macOS.

### Architecture cuối

- `BoundedProcessRunner` stream stdout/stderr đồng thời qua pipe, giữ preview bounded, không gom toàn bộ output bằng `Command::output()` trong production Git paths.
- Có timeout, `CancellationToken`, `killOnLimit`, kill process group/tree và reap child trước khi trả kết quả.
- Git subprocess dùng stdin null, tắt pager/prompt tương tác và giữ argument-safe execution bằng `Command.args` + `--` cho path.
- Output lớn có thể externalize sang managed artifact; import file tạm được stream vào `BlobStore`, kiểm tra size/SHA-256, áp quota và đăng ký `artifact_registry` theo task.
- Progress của runner được throttle theo byte/thời gian, không queue unbounded; metrics cuối có `stdoutBytes`, `stderrBytes`, `firstOutputMs`, `artifactBytes`, `elapsedMs`, `timedOut`, `cancelled`.

### Structured Git contract

- `git_status`: porcelain v2 + branch metadata, ordinary/rename/copy/unmerged/untracked, index/worktree status, non-UTF-8 path có `pathBytesBase64`, item limit, signed cursor, `nextCursor`, `hasMore`.
- `git_log`: structured commit/shortCommit/author/authoredAt/subject, limit, signed cursor, `hasMore`.
- `git_branch`: ref/name/objectId/current/upstream, limit/cursor/hasMore.
- `git_commit`: phase rõ (`staging`, `commitHooksIncluded`, committed tương đương), không báo success khi timeout/cancel, trả commit hash khi thành công.
- Parser đã được tách khỏi `git_service.rs`; `git_service.rs` hiện 458 dòng.

### Artifact range behavior

- `task_artifact_read` hỗ trợ `offset` + `maxBytes` và trả `content`, `offset`, `nextOffset`, `hasMore`/truncation metadata.
- Managed artifact vẫn task-scoped, không load toàn artifact vào RAM; implementation seek tới offset và đọc bounded, giữ UTF-8 boundary an toàn.
- Lazy activity detail đọc nhiều bounded range để tái tạo JSON artifact tối đa 2 MiB thay vì giả định một chunk đủ lớn; regression test `large_tool_content_is_absent_from_sqlite_and_realtime` PASS.

### Adversarial/security coverage đã xác minh

- Large Git status spill không vượt inline cap.
- Binary diff bounded và argument-safe.
- External diff bị vô hiệu hóa.
- Corrupt repository / index lock fail có kiểm soát, không panic.
- Hanging pre-commit hook timeout và bị reap, không tạo commit.
- Commit thành công trả structured commit hash.
- Revision bắt đầu bằng `-` và control/newline injection bị reject; path separator `--` vẫn được giữ.
- Unix timeout giết cả child/grandchild process group.
- Artifact hard cap giữa stream có `artifactLimit`; khi `killOnLimit=false` producer vẫn được drain tới exit, khi `true` producer bị dừng.
- Timeline/realtime không persist full large tool output; chỉ bounded projection + artifact reference.

### Benchmark cuối trên macOS

`git_diff_10mib_100mib_1gib_reports_streaming_metrics` — PASS:

- 10 MiB: stdoutBytes=10,680,142; artifactBytes=10,680,142; firstOutputMs=29 ms; elapsedMs=506 ms; throughput≈20.13 MiB/s; reason=`contentExternalized`.
- 100 MiB: stdoutBytes=106,799,811; artifactBytes=106,799,811; firstOutputMs=96 ms; elapsedMs=4,899 ms; throughput≈20.79 MiB/s; reason=`contentExternalized`.
- 1 GiB: stdoutBytes=1,093,629,168; artifactBytes=1,073,741,824; firstOutputMs=106 ms; elapsedMs=49,595 ms; throughput≈20.65 MiB/s; reason=`artifactLimit`; process vẫn exit thành công và preview bounded.
- `/usr/bin/time -l` cho diff benchmark: maximum resident set size=181,141,504 bytes; peak memory footprint=85,115,504 bytes.

`git_status_100k_entries_reports_first_page_and_metrics` — PASS:

- entries=100,000; stdoutBytes=2,800,042; artifactBytes=2,800,042; firstOutputMs=109 ms; elapsedMs=234 ms; reason=`contentExternalized`.
- `/usr/bin/time -l`: maximum resident set size=110,821,376 bytes; peak memory footprint=85,951,088 bytes.

`git_diff_cancellation_latency_is_bounded` — PASS:

- totalMs=64 ms; cancelLatencyMs=14 ms.

### Validation cuối

Đều PASS trên macOS:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

Full workspace suite sau fix lazy artifact detail: 0 failed. Warning còn thấy ở `crates/chatcmd-runtime/tests/search_perf.rs` là unused import của benchmark Plan 06, ngoài phạm vi Plan 15 và không ảnh hưởng kết quả validation.

Không còn mục công việc chưa hoàn thiện của Plan 15.
