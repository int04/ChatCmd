# Plan 06 — Nâng cấp `fs_search` thành search v2 có budget, cursor và streaming file scan

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy nâng cấp `fs_search` để tìm nội dung hiệu quả trên repository lớn và file text lớn. Tool mới phải scan streaming, có literal/regex rõ ràng, context lines, cursor continuation, budget/cancellation và result envelope chuẩn. Giữ adapter tool cũ. Không commit.

## Ưu tiên

**P1 — trọng yếu cho monorepo.** Implementation hiện có ignore rules và early-stop theo số match, nhưng mỗi file được đọc thành `String`, không có cursor/resume và không giới hạn tổng bytes/time/files đã scan.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem_search.rs:20-128`:
  - dùng `WalkBuilder` và `follow_links(false)`;
  - cap `max_results` 2.000;
  - bỏ file lớn hơn `max_file_bytes`;
  - `fs::read_to_string(entry.path())` đọc toàn file được chọn;
  - case-insensitive gọi `line.to_lowercase()` cho từng dòng;
  - không có cursor, total byte budget, timeout hay cancellation trong loop.
- `src/runtime_host/inputs.rs:169-180` có `query`, `caseSensitive`, `maxResults`, `maxFileBytes`, `includeIgnored`, `exclude`.
- `src/runtime_host/filesystem_dispatch.rs:48-105` publish progress qua event; path match được dedupe bằng `HashSet`, nhưng không có throttle theo thời gian/output budget chung.
- `crates/chatcmd-mcp/src/lib.rs:285-315` chưa mô tả literal/regex/context/cursor.

## Mục tiêu

1. Scan file theo buffer/line/chunk, không `read_to_string` toàn file.
2. Hỗ trợ `literal` và `regex` với semantics typed; case-insensitive Unicode/ASCII được chọn rõ.
3. Trả line, column/byte offset, match range và context có giới hạn.
4. Có budget tổng: timeout, files scanned, bytes scanned, matches, output bytes, max per-file bytes.
5. Có cursor continuation bind với root/query/options và repository snapshot/version strategy.
6. Cancellation dừng walker và file scanner thực sự.
7. Binary/invalid UTF-8/permission errors được thống kê và cảnh báo, không im lặng nuốt mọi lỗi.
8. Progress realtime được throttle/coalesce, không tạo event storm.

## Contract đề xuất

```json
{
  "path": ".",
  "query": "read_text_range",
  "mode": "literal",
  "caseSensitive": true,
  "wordBoundary": false,
  "include": ["**/*.rs"],
  "exclude": ["target/**"],
  "includeIgnored": false,
  "contextBefore": 2,
  "contextAfter": 2,
  "maxMatchesPerFile": 50,
  "cursor": null,
  "limit": 200,
  "budget": {
    "timeoutMs": 15000,
    "maxFilesScanned": 100000,
    "maxBytesScanned": 536870912,
    "maxOutputBytes": 524288,
    "maxFileBytes": 67108864
  }
}
```

Mỗi match nên có:

```json
{
  "path": "crates/chatcmd-runtime/src/filesystem.rs",
  "line": 145,
  "column": 18,
  "byteOffset": 4312,
  "matchText": "read_text_range",
  "lineText": "...",
  "contextBefore": ["..."],
  "contextAfter": ["..."],
  "lineTruncated": false
}
```

Result dùng plan 02, gồm `nextCursor`, `hasMore`, `truncationReason`, `filesScanned`, `bytesScanned`, `filesSkippedBySize`, `binaryFilesSkipped`, `errorsSkipped`, `elapsedMs`.

## Thiết kế scan

- Dùng `BufReader`/streaming searcher; cân nhắc crates từ hệ sinh thái ripgrep như `grep-searcher`, `grep-matcher`, `regex-automata`, `memchr` nếu phù hợp license/dependency.
- Compile matcher một lần.
- Literal ASCII case-insensitive tránh allocate lowercase mỗi dòng; Unicode mode phải tài liệu hóa chi phí.
- Giữ ring buffer nhỏ cho `contextBefore`; chỉ đọc thêm đủ `contextAfter` sau match.
- Line/snippet có cap byte riêng và cắt ở UTF-8 boundary.
- Detect binary bằng NUL/sample policy; caller có option `binaryMode=skip|error|raw` nếu cần.
- Permission/read error mặc định collect warning bounded; có `failOnError` tùy chọn nếu cần strict.

## Cursor và consistency

Cursor phải bind với:

- canonical root;
- normalized query/mode/case/options;
- traversal/ignore configuration;
- workspace/repository version hoặc snapshot ID;
- vị trí walker + offset file hiện tại nếu resume giữa file.

Nếu tree thay đổi:

- strict mode trả `cursorStale`; hoặc
- best-effort mode tiếp tục và trả warning `resultsMayHaveChanged`.

Không được âm thầm quét lại từ đầu khi cursor hết hạn vì sẽ tạo duplicate và chi phí khó đoán.

## Các bước triển khai

1. Tách search request/result/matcher/walker/progress sang module nhỏ dưới 500 dòng.
2. Tích hợp shared ignore/walker plan 07.
3. Implement streaming literal matcher trước, sau đó regex typed.
4. Thêm context ring buffer và match offset metadata.
5. Thêm `SearchBudget` dùng abstraction plan 16.
6. Kiểm tra cancellation trong walker và file read loop.
7. Implement cursor state/TTL/ownership hoặc stateless cursor khả thi.
8. Coalesce progress theo thời gian, ví dụ tối đa 2–5 event/giây, và chỉ gửi counters + current path bounded.
9. Tích hợp result envelope plan 02.
10. Giữ `fs_search` cũ map sang mode literal, context 0, cursor null; giới hạn an toàn.
11. Cập nhật MCP schema, docs và UI timeline.

## Edge cases bắt buộc

- Query rỗng, query rất dài, regex invalid.
- CRLF/LF/mixed, UTF-8 BOM, invalid UTF-8.
- Dòng dài hàng chục MB.
- Match qua chunk boundary.
- Nhiều match trên cùng dòng và overlapping semantics.
- Binary file, sparse file, file đổi/truncate khi đang scan.
- `.gitignore` negate, include/exclude conflict.
- Symlink loop/trỏ ngoài workspace.
- Permission denied và file biến mất.
- Cancellation lúc đang đọc file lớn.

## Test bắt buộc

- Search literal/regex case-sensitive và insensitive.
- Match đúng line/column/byte offset qua chunk boundary.
- Context trước/sau đúng, không giữ toàn file.
- Tổng `bytesScanned` không vượt budget quá một buffer được định nghĩa.
- Hết files/bytes/time/output budget trả reason đúng và cursor nếu resume được.
- Cursor continuation không trùng/thiếu trên tree bất biến.
- Cursor scope/query mismatch bị từ chối.
- Invalid UTF-8/binary/permission errors có counter/warning đúng.
- Progress event count bị throttle trong fixture 100.000 file.
- Cancellation kết thúc worker nhanh, không còn task blocking chạy ngầm.
- Compatibility test cho `fs_search` cũ.

## Benchmark bắt buộc

- Repository fixture 10.000, 100.000 file; cold/warm run.
- File text 10 MB/100 MB với match đầu, giữa, cuối và không match.
- Đo time-to-first-match, throughput MB/s, peak memory, total event count.
- So sánh với implementation cũ; không yêu cầu vượt ripgrep nhưng phải bounded và không OOM.

## Tiêu chí nghiệm thu

- Không còn `fs::read_to_string` toàn file trong đường search mới.
- Có total resource budget và cancellation cooperative.
- Literal/regex semantics rõ và schema typed.
- Result có cursor, context, offsets và usage.
- Progress không flood WebSocket/SQLite.
- Ignore behavior thống nhất với find/index/watcher.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- Matcher/walker/cursor design.
- Contract request/result cuối.
- File đã đổi.
- Error/binary/encoding behavior.
- Test/benchmark và số liệu throughput/RAM/events.
- Compatibility behavior của tool cũ.
