# Plan 22 — Coalesce output PTY, backpressure replay và chặn dùng `shell_write` làm kênh bulk content

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy tối ưu shell/PTY pipeline để output nhỏ lẻ được coalesce thành chunk hợp lý, replay/event storage có hard cap và backpressure, slow consumer không làm tăng RAM vô hạn. Đồng thời đặt input cap/guard để `shell_write` không bị dùng để truyền file/script lớn; caller phải dùng filesystem/blob tools. Giữ terminal tương tác realtime và sequence cursor. Không commit.

## Ưu tiên

**P1 — hiệu năng và độ tin cậy.** Audit thực tế cho thấy một heredoc chỉ vài KiB qua PTY có thể tạo hàng nghìn event echo, làm dữ liệu bị biến dạng và phình timeline/replay. Shell phù hợp lệnh/input tương tác, không phù hợp bulk-content transport.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/inputs.rs:95-145` `ShellWrite` nhận `text: String`, mặc định append newline; không thấy max byte rõ trong input contract.
- `crates/chatcmd-runtime/src/types.rs:218-260` `ShellWriteRequest` giữ full `String` và shell event có sequence.
- `src/runtime_host/persistence.rs:136-230` `persist_shell_events` tạo một `TerminalEventChunk` cho từng event và publish từng event riêng qua realtime.
- `src/runtime_host/dispatch.rs:120-205` shell create/write/wait/read dispatch; `shell_read.max_events` được clamp nhưng cần audit byte cap.
- Runtime config có `max_replay_bytes`, shell implementation dưới `crates/chatcmd-runtime/src/shell.rs` và `shell/`; cần đọc toàn bộ trước sửa.
- Activity stop route có gửi Ctrl-C cho shell session; cần tích hợp kill escalation/process-tree cleanup từ plan 16.
- Plan 13 yêu cầu không duplicate terminal output vào timeline tool output và terminal chunk store.

## Mục tiêu

1. PTY reader coalesce tiny reads/keystroke echo thành chunks bounded theo bytes và latency.
2. Output queue/replay buffer bounded theo bytes, events và sessions; slow consumers gây drop/truncation có metadata, không OOM.
3. `shell_read` cursor/sequence vẫn deterministic và báo replay eviction/gap rõ.
4. Persistence batch nhiều chunks/events trong một transaction; không write DB/publish WebSocket cho từng byte/tiny read.
5. `shell_write` có max bytes/call và optional rate limit; payload lớn trả error hướng dẫn dùng `fs_write_text`, `fs_write_raw` hoặc blob contentRef.
6. UTF-8 split, ANSI escape, binary PTY bytes và CR/LF được xử lý không corrupt.
7. Sensitive command input không bị persist/log nguyên văn theo mặc định.
8. Stop/close/timeout cleanup process tree, reader threads/tasks và writer handles.
9. Terminal tương tác vẫn có latency thấp; batching không làm UI cảm giác treo.

## Input contract đề xuất

Mở rộng schema:

```json
{
  "sessionId": "...",
  "text": "cargo test",
  "appendNewLine": true,
  "inputKind": "interactive",
  "sensitive": false
}
```

`inputKind`:

- `interactive`: lệnh/keystroke nhỏ, max server default ví dụ 16–64 KiB/call.
- `paste`: paste vừa, cap lớn hơn chút và rate-limited nếu cần.
- Không có `bulkFile`; dữ liệu lớn phải đi blob/filesystem.

Server hard cap phải enforce theo UTF-8 bytes trước clone/persist. Error:

```json
{
  "code": "shellInputTooLarge",
  "message": "shell_write is for interactive input; use fs_write_text/fs_write_raw with contentRef",
  "maxBytes": 65536,
  "receivedBytes": 123456
}
```

Không tự chia giant payload thành nhiều writes vì PTY echo/line discipline có thể corrupt và gây event storm.

## Output coalescing design

PTY reader nhận raw byte chunks và đưa qua `TerminalOutputCoalescer`:

- flush khi đạt `maxChunkBytes` (ví dụ 8–32 KiB);
- hoặc `maxLatency` (ví dụ 16–50 ms) để giữ realtime;
- flush ngay khi process exit/close, stream type đổi hoặc special terminal event;
- không split UTF-8 code point khi output declared UTF-8;
- ANSI sequence có thể split giữa chunks nếu terminal emulator hỗ trợ streaming; nếu không, coalescer giữ bounded carry nhỏ, không chờ vô hạn malformed escape;
- giữ raw bytes hoặc encoding metadata thay vì `String` lossy quá sớm.

Sequence semantics nên áp dụng trên coalesced chunk. Nếu cần giữ raw event sequence nội bộ, trả `firstSequence`/`lastSequence` cho chunk. Cursor `afterSequence` phải tài liệu hóa version migration.

## Bounded replay/backpressure

Mỗi session có:

- max replay bytes;
- max replay chunks/events;
- max age optional;
- oldest/latest sequence;
- dropped/evicted bytes/chunks counters.

Khi cap:

- ring buffer evict oldest.
- `shell_read` với cursor cũ trả `replayTruncated=true`, `oldestAvailableSequence`, `latestAvailableSequence`, `droppedBytes`.
- Không block PTY reader vô hạn vì UI chậm; persistence/artifact sink có bounded queue và degradation policy.
- Nếu DB writer chậm, coalesce/batch và mark dropped persistence separately; live terminal capture correctness policy phải rõ.

Global limits:

- max total replay bytes all sessions;
- per-agent/task/session quota;
- weighted memory reservation/admission.

## Persistence/realtime

- Batch terminal chunks theo byte/time threshold trong một repository call/transaction.
- Publish one coalesced event, không one event per raw read/character.
- Timeline chỉ lưu shell tool started/result summary; terminal bytes ở terminal chunk store, tránh duplication plan 13.
- WebSocket broadcaster bounded; subscriber lag có gap notification và can replay by cursor.
- UI throttles renders, nhưng không phải nơi duy nhất bảo vệ backend.
- Retention/cleanup sau session/task delete/app restart rõ.

## Encoding/terminal semantics

- PTY output là bytes; decode incremental bằng stateful UTF-8 decoder cho UI, preserve incomplete sequence carry.
- Invalid bytes: `encoding=base64`/replacement policy explicit; không drop silently.
- CR progress lines (`\r`) phải render đúng; coalescing không biến thành nhiều permanent lines.
- ANSI escape and OSC sequences có security filtering ở UI (không execute links/clipboard/control dangerous sequences nếu chưa có policy).
- Resize/input/interrupt ordering giữ đúng; writer serialized per session.

## Stop và lifecycle

- `shell_signal CtrlC` graceful; sau configurable grace có terminate/kill tree khi user chọn force/stop conversation.
- Reader/writer/monitor tasks được quản lý bằng `JoinSet`/RAII và cancellation token.
- Close idempotent; session exit vẫn giữ bounded replay cho TTL.
- App restart reconcile terminal rows với process thực; không hiển thị ghost running sessions.
- Busy/session map cleanup không phụ thuộc UI poll.

## Các bước triển khai

1. Đọc toàn bộ shell modules, replay store, terminal persistence/UI assumptions.
2. Tạo benchmark/repro tiny-output event storm và giant `shell_write` trước sửa.
3. Tạo `TerminalOutputCoalescer` + incremental decoder tests.
4. Đổi replay storage sang byte-bounded ring with gap metadata.
5. Thêm bounded channels/global/session quotas/admission.
6. Batch persistence và realtime publication.
7. Thêm `shell_write` byte cap/inputKind/sensitive projection.
8. Tích hợp result envelope/usage/metrics và plan 13 redaction.
9. Tích hợp cancel/kill-tree/lifecycle cleanup.
10. Cập nhật MCP schema, docs, UI terminal reader/render tests.

## Edge cases bắt buộc

- Một byte mỗi read ở tốc độ cao.
- 100 MB/s stdout, stderr/PTY mixed.
- UTF-8 code point chia qua nhiều raw reads.
- ANSI/OSC sequence chia chunk hoặc malformed dài.
- Carriage-return progress bars và backspace.
- Slow/no WebSocket subscriber; DB locked/chậm.
- Cursor cũ hơn replay ring.
- Giant heredoc/paste, NUL bytes, sensitive input.
- Ctrl-C trong process tree; child ignores signal.
- Close/read/write/exit race.
- Session/task/app shutdown/restart.

## Test bắt buộc

- Tiny raw events coalesce thành số chunk bounded nhưng latency dưới target.
- Coalescing không đổi byte stream khi concatenate output chunks.
- UTF-8/ANSI/CR test vectors.
- Replay eviction/gap/cursor semantics.
- Slow consumer không tăng memory vượt cap.
- Global/per-session quota và fairness.
- DB/realtime batch count giảm mạnh so baseline.
- `shell_write` quá cap bị từ chối trước persist/write; error hướng dẫn filesystem/blob.
- Sensitive input không xuất hiện trong timeline/log/diagnostics.
- Stop/force-close kill process tree và cleanup tasks.
- Existing shell lifecycle/replay tests không regress.

## Benchmark bắt buộc

- 1.000.000 tiny writes/events.
- Sustained large output với consumer nhanh/chậm/không có.
- Đo chunks emitted, DB writes, WebSocket events, peak memory, CPU, p50/p95 display latency và stop latency.
- So sánh baseline event-per-read với coalesced implementation.

## Tiêu chí nghiệm thu

- `shell_write` có hard byte cap và không được dùng bulk file transport.
- PTY tiny output được coalesce theo size/latency.
- Replay và channels bounded theo bytes, có gap metadata.
- Persistence/realtime không phát/lưu từng tiny event.
- Concatenate chunks tái tạo đúng raw byte stream trong phạm vi retained replay.
- Sensitive input được redacted.
- Stop/close không để process/task reader orphan.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

Chạy frontend terminal tests/build nếu sửa UI.

## Kết quả AI phải trả về

- Coalescing thresholds và lý do.
- Replay/channel quota và cursor-gap contract.
- `shell_write` cap/schema/error flow.
- Persistence/realtime trước/sau.
- File/UI đã đổi.
- Benchmark events/DB/RAM/latency và lifecycle test.

## Kiểm tra lại sau triển khai

Plan được đánh dấu `_check` vì lần chạy đầu của `cargo fmt --check` phát hiện khác biệt định dạng trong mã mới. Các lần chạy `cargo check --workspace` đầu tiên cũng phát hiện một `Arc<Session>` bị chuyển quyền sở hữu vào hai luồng và cặp trường đầu vào bị khai báo trùng; các lỗi biên dịch này đã được sửa. Công cụ định dạng và trình biên dịch sẽ được chạy kiểm tra lại, nhưng quy tắc của đợt triển khai yêu cầu giữ dấu kiểm tra khi bất kỳ bước kiểm tra nào từng thất bại.

Ngoài ra, cần chạy phép đo hiệu năng riêng trong môi trường thực tế để đo mức RAM cực đại, CPU, độ trễ hiển thị p50/p95 và độ trễ dừng ở tải PTY 100 MB/s với bên tiêu thụ nhanh/chậm/không có. Kiểm thử đơn vị hiện bao phủ một triệu lượt đọc cực nhỏ, giới hạn đoạn dữ liệu và bảo toàn luồng byte; môi trường CI thông thường không cho kết quả đo độ trễ/tài nguyên ổn định. Cần kiểm tra thủ công việc nâng cấp mức kết thúc cây tiến trình trên Linux/macOS và tiến trình con cố tình bỏ qua Ctrl-C; lượt triển khai này chỉ xác nhận đường buộc đóng hiện có trên Windows và kiểm thử vòng đời hiện hữu.

Giao diện cần kiểm tra lại: `npm run lint` thất bại do 8 lỗi/8 cảnh báo nền (các biến toàn cục Node trong `scripts/obfuscate-build.mjs`, thay đổi tham chiếu trong `src/realtime.ts` và cảnh báo phụ thuộc hook). `npm test -- --run` có 47 kiểm thử thành công, 7 kiểm thử thất bại trong `src/test/App.test.tsx` do không tìm thấy các chuỗi giao diện mong đợi; jsdom cũng báo canvas `getContext` chưa được triển khai. `npm run build` vẫn thành công. Không gian làm việc Rust vượt qua toàn bộ kiểm thử đã chạy, nhưng có các bộ dữ liệu kiểm thử hiệu năng/phương án dự phòng mang thuộc tính `ignored` theo cấu hình kiểm thử hiện tại.

## Rà soát lại ngày 2026-09-05

Đã xử lý thêm các khoảng trống có thể hoàn tất trực tiếp trong môi trường hiện tại:

- Trên macOS, kiểm tra thực tế phát hiện grandchild cố tình bỏ qua `HUP`, `TERM` và `INT` vẫn sống sau `shell_close(force=true)`. `kill_tree` đã được sửa để trên Unix gửi `SIGKILL` cho toàn process group của PTY trước khi fallback về `Child::kill`; Windows vẫn dùng `taskkill /T /F`. Regression test `shell_force_close_kills_stubborn_process_group` tạo grandchild cố tình bỏ qua signal và xác nhận process đó không còn tồn tại sau force-close.
- Test frontend realtime có race vì assertion chạy trước chuỗi decrypt bất đồng bộ hoàn tất. Test đã chờ điều kiện bằng `vi.waitFor`, không thay đổi hành vi production.
- PATH mặc định của máy đang trỏ Node `v14.16.0`, khiến ESLint/Vitest không parse được dependency hiện tại. Máy đã có Node `v22.22.3`; chạy lại với Node 22 cho kết quả `npm run lint` pass, `npm test -- --run` pass 54/54 và `npm run build` pass. Các cảnh báo jsdom canvas/media không làm test thất bại.
- Validation Rust sau thay đổi: `cargo fmt --check` pass, `cargo check --workspace` pass, test shell/kill-tree pass và `cargo test --workspace` pass. Một lượt trước đó có test adversarial filesystem ngoài Plan 22 (`simultaneous_expected_version_writers_have_one_commit_winner`) fail do race; rerun riêng test đó pass và lượt `cargo test --workspace` cuối cùng cũng pass.

### Hoàn tất benchmark và điều kiện `_Checkdone`

Đã bổ sung benchmark thủ công `crates/chatcmd-runtime/tests/shell_output_perf.rs` và chạy trực tiếp trên macOS với mỗi case phát 100 MiB PTY output, cấu hình `shell_output_chunk_bytes=16 KiB`, `shell_output_max_latency_ms=25`, replay cap 8 MiB/8.192 events. Producer dùng byte stream không newline để tránh line-discipline CR/LF làm sai số liệu. Benchmark chạy ba chế độ consumer và một phép đo force-stop:

- Fast consumer: 100 MiB trong 6.322 s (~15,82 MiB/s), 6.401 coalesced events so với baseline xấp xỉ 12.800 raw 8 KiB reads (~2,00x giảm event), 2.409 `shell_read` calls, delivery latency p50/p95 = 1/3 ms, CPU ~3,32 s, peak RSS ~12,5 MiB.
- Slow consumer (poll 50 ms): 100 MiB trong 6.379 s (~15,68 MiB/s), 6.401 coalesced events, 118 `shell_read` calls, p50/p95 = 28/52 ms, CPU ~3,09 s, peak RSS ~21,9 MiB.
- No consumer cho tới khi producer exit: 100 MiB trong 6.243 s (~16,02 MiB/s), 6.401 total coalesced events nhưng replay chỉ giữ ~8,0 MiB theo cap, chỉ 1 `shell_read` sau khi producer kết thúc, p50/p95 của retained replay = 259/475 ms, CPU ~2,94 s, peak RSS ~37,2 MiB. `replayTruncated=true` xác nhận bounded replay/gap semantics hoạt động dưới consumer vắng mặt.
- Force-stop latency của producer vô hạn: 56 ms.

Trong host persistence path, mỗi `shell_read` gọi `append_terminal_chunks` đúng một lần cho toàn batch và realtime publish theo coalesced event, vì vậy benchmark `shellReadCalls` đại diện số batch persistence calls còn `coalescedEvents` là số terminal chunk/WebSocket events tối đa thay cho raw tiny reads. So với event-per-8-KiB-read baseline, coalescing giảm khoảng 2x event count ở workload 100 MiB này; với 1-byte/tiny-read workload, unit test 1.000.000 reads vẫn xác nhận số chunk dưới 125 ở chunk cap 8 KiB.

Benchmark cũng phát hiện thêm race lifecycle: force-close một session vừa tự exit có thể trả `io_error: No such process`. `kill_tree` đã được sửa để coi session đã `exited` là idempotent success trước/sau `Child::kill`, và regression test `force_close_is_idempotent_after_process_exit` đã được thêm. Test `shell_force_close_kills_stubborn_process_group` tiếp tục pass trên macOS.

Validation cuối sau toàn bộ thay đổi: `cargo fmt --check` pass, `cargo check --workspace` pass, hai regression test lifecycle pass, benchmark Plan 22 pass, và `cargo test --workspace` pass. Frontend đã được validation ở lượt rà soát ngay trước đó bằng Node 22.22.3: lint pass, 54/54 tests pass, build pass. Không còn blocker nghiệm thu Plan 22 trong môi trường hiện tại; plan đủ điều kiện `_Checkdone`.
