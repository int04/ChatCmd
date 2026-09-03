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

Plan được đánh dấu `_check` vì lần chạy đầu của `cargo fmt --check` phát hiện khác biệt định dạng trong mã mới. Các lần `cargo check --workspace` đầu cũng phát hiện một `Arc<Session>` bị move vào hai thread và cặp field input bị khai báo trùng; các lỗi compile này đã được sửa. Formatter và compiler sẽ được chạy kiểm tra lại, nhưng quy tắc của đợt triển khai yêu cầu giữ dấu kiểm tra khi bất kỳ check nào từng fail.

Ngoài ra cần chạy benchmark production riêng để đo peak RAM, CPU, p50/p95 display latency và stop latency ở tải PTY 100 MB/s với consumer nhanh/chậm/không có. Unit test hiện bao phủ một triệu tiny reads, giới hạn chunk và bảo toàn byte stream; môi trường CI thông thường không cho kết quả benchmark latency/tài nguyên ổn định. Cần kiểm tra thủ công process-tree escalation trên Linux/macOS và child cố tình bỏ qua Ctrl-C; lượt triển khai này chỉ xác nhận đường force-close hiện có trên Windows và lifecycle test hiện hữu.

Frontend cần kiểm tra lại: `npm run lint` fail do 8 lỗi/8 warning baseline (Node globals trong `scripts/obfuscate-build.mjs`, ref mutation trong `src/realtime.ts` và hook dependency warnings). `npm test -- --run` có 47 test pass, 7 test fail trong `src/test/App.test.tsx` do không tìm thấy các UI text mong đợi; jsdom cũng báo canvas `getContext` chưa được triển khai. `npm run build` vẫn pass. Rust workspace pass toàn bộ test được chạy, nhưng có các perf/fallback fixture `ignored` theo cấu hình test hiện tại.
