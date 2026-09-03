# Plan 05 — Nâng cấp `fs_find` cho traversal lớn, ignore rules, early-stop và cursor

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy viết lại `fs_find` để tìm file/folder hiệu quả trên monorepo lớn. Tool phải dùng traversal có ignore rules thống nhất, dừng thật khi đủ kết quả/budget, hỗ trợ cursor continuation và cancellation. Không sửa `fs_search` ngoài abstraction dùng chung. Không commit.

## Ưu tiên

**P1.** `fs_find` hiện phù hợp cây nhỏ nhưng có thể quét toàn bộ repository dù đã đủ kết quả và dễ đi vào thư mục generated lớn.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:262-289`:
  - pattern bị biến thành `pattern.trim_matches('*').to_lowercase()`;
  - callback chỉ ngừng push khi `found.len()` đạt limit, nhưng traversal `visit` vẫn tiếp tục.
- `crates/chatcmd-runtime/src/filesystem.rs:356-389` có helper recursive `visit` dùng `fs::read_dir`, bỏ symlink nhưng không áp dụng `.gitignore`/default excludes và không có cooperative cancellation.
- `src/runtime_host/inputs.rs:181-194` chỉ có `path`, `pattern`, `maxResults`, `maxDepth`.
- `crates/chatcmd-mcp/src/lib.rs:300-325` mô tả “find workspace paths”, chưa nói semantics pattern, ignore, continuation hay budget.

## Mục tiêu

1. Dùng traversal iterator/walker có thể early-stop thật khi đủ kết quả hoặc hết budget.
2. Semantics pattern rõ ràng: `literal`, `glob`, tùy chọn `regex`; không giả vờ hỗ trợ wildcard bằng cách chỉ bỏ `*` hai đầu.
3. Dùng ignore policy chung với search/watcher/indexer từ plan 07.
4. Có filter theo `entryType`, extension, hidden, depth và include/exclude glob.
5. Có `nextCursor` hoặc stateful continuation có scope/version rõ.
6. Cancellation, timeout, max entries scanned và max metadata calls được enforce trong loop.
7. Symlink không được follow mặc định; nếu sau này cho follow phải chống loop và không thoát workspace scope.

## Contract đề xuất

```json
{
  "path": ".",
  "pattern": "**/*filesystem*.rs",
  "patternMode": "glob",
  "caseSensitive": false,
  "entryTypes": ["file"],
  "maxDepth": 64,
  "includeIgnored": false,
  "exclude": ["target/**", "node_modules/**"],
  "cursor": null,
  "limit": 200,
  "budget": {
    "timeoutMs": 10000,
    "maxEntriesScanned": 100000,
    "maxMetadataCalls": 10000
  }
}
```

Kết quả dùng envelope plan 02:

```json
{
  "items": [{ "path": "crates/.../filesystem.rs", "entryType": "file" }],
  "nextCursor": "...",
  "hasMore": true,
  "truncationReason": null,
  "usage": { "entriesScanned": 1240, "elapsedMs": 12 }
}
```

## Thiết kế traversal

- Ưu tiên `ignore::WalkBuilder` để dùng `.gitignore`, hidden/default rules và `follow_links(false)` nhất quán với search.
- Tạo abstraction `WorkspaceWalkerOptions` dùng chung thay vì copy danh sách ignore.
- Với `glob`, compile pattern một lần trước traversal; invalid pattern fail trước khi scan.
- Với `regex`, compile có size/time limits hoặc dùng regex engine an toàn khỏi catastrophic backtracking.
- Callback/iterator phải trả control `Continue/Stop`; không chỉ ngừng push.
- Không gọi metadata cho mọi entry nếu `DirEntry.file_type()` và path đủ đáp ứng filter.
- Nếu dùng parallel walker, channel phải bounded, kết quả deterministic hoặc tài liệu hóa order; cancellation phải đóng worker cleanly.

## Cursor strategy

Do traversal recursive không dễ resume portable chỉ bằng path cuối, chọn một chiến lược rõ:

- Server-side iterator/snapshot ID có TTL; hoặc
- Cursor chứa DFS stack đã serialize và bind với root/options/version; hoặc
- Dựa vào repository index ở plan 20.

MVP có thể dùng stateful cursor cache bounded với:

- max active cursors;
- TTL;
- per-agent/task ownership;
- cleanup khi task dừng/app shutdown;
- lỗi `cursorExpired` thay vì quét lại âm thầm.

Không dùng cursor client-controlled để mở path ngoài scope.

## Các bước triển khai

1. Tách `fs_find` sang module riêng dưới 500 dòng.
2. Tạo typed enums cho `PatternMode`, `EntryTypeFilter`, order và truncation reason.
3. Xây shared walker/ignore abstraction hoặc tích hợp plan 07 nếu đã có.
4. Implement literal + glob trước; regex chỉ thêm nếu có test/budget đầy đủ.
5. Enforce limit/budget/cancellation trong traversal loop.
6. Implement cursor ownership/scope/TTL.
7. Giữ adapter tool cũ:
   - mapping pattern cũ sang literal-contains;
   - cap an toàn;
   - trả deprecation warning nếu contract cho phép.
8. Cập nhật schema MCP và docs với ví dụ literal/glob.
9. Bổ sung usage/progress event có throttle; không phát một WebSocket event cho từng file.

## Edge cases bắt buộc

- Pattern `*`, `**/*.rs`, ký tự đặc biệt, Unicode và case sensitivity.
- Root là file, root là directory rỗng, broken path.
- `.gitignore` negate pattern.
- Hidden directory, `target`, `node_modules`, `.git`.
- Symlink loop và symlink trỏ ra ngoài workspace.
- Permission denied ở một subtree.
- Directory thay đổi giữa các page.
- Cursor bị dùng bởi task/agent khác.
- Hết timeout sau khi có partial result.

## Test bắt buộc

- Cây 100.000+ path sinh tự động, kết quả đầu dừng sớm.
- Instrumentation chứng minh walker không tiếp tục sau limit trong single-thread mode; worker parallel được cancel/join.
- Ignore/default/exclude/includeIgnored đúng.
- Literal/glob/regex semantics có test riêng.
- `maxDepth` chính xác tại boundary.
- Cancellation, timeout, maxEntriesScanned.
- Cursor continuation không trùng/thiếu khi cây bất biến.
- Cursor expired/scope mismatch/version mismatch.
- Không follow symlink mặc định.
- Tool cũ vẫn hoạt động theo semantics cũ đã tài liệu hóa.

## Tiêu chí nghiệm thu

- Không còn helper recursion quét tiếp sau khi đã đủ result trong đường chạy mới.
- Pattern semantics được schema hóa, không dùng `trim_matches('*')` làm parser wildcard.
- Ignore policy dùng chung, không có một danh sách hard-code mới.
- Result có continuation và usage.
- Cancellation dừng worker hữu hạn và không rò cursor/task.
- Benchmark trên cây lớn cho thấy time-to-first-page thấp và số entry scan bị giới hạn.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- Contract và pattern semantics cuối.
- Cursor strategy/ownership/TTL.
- Module/file đã đổi.
- Cách early-stop và cancel worker.
- Test/benchmark, entries scanned và elapsed time.
- Compatibility behavior của `fs_find` cũ.
