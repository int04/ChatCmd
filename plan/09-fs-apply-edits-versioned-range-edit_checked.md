# Plan 09 — Thêm `fs_apply_edits` để sửa range có version, atomic và conflict-safe

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thêm tool `fs_apply_edits` dùng để sửa một hoặc nhiều range trong file text lớn mà không phải gửi/ghi lại toàn file. Tool bắt buộc hỗ trợ optimistic concurrency qua `expectedVersion`, validate edits không chồng lấn, dry-run và atomic commit. Giữ `fs_replace_text` tương thích nhưng định hướng nó thành adapter/legacy tool. Không commit.

## Ưu tiên

**P0 — tool sửa file cốt lõi.** Đây là giải pháp chính để AI sửa file lớn an toàn và tránh ghi đè thay đổi đồng thời.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:209-260` `replace_text`:
  - `read_to_string` toàn file;
  - đếm occurrences;
  - tạo `updated = content.replace(...)` toàn file;
  - gọi `write_text(..., overwrite=true)`.
- `src/runtime_host/inputs.rs:223-239` `ReplaceTextInput` chỉ có `path`, `oldText`, `newText`, `expectedOccurrences`; không có version/hash/range.
- `crates/chatcmd-mcp/src/lib.rs:330-365` schema MCP tương ứng.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:30-76` nhận toàn bộ bytes trong RAM và thay file.
- `src/runtime_host/filesystem_dispatch.rs:108-151` snapshot before/after có thể nhân bản content.

Plan này phụ thuộc hoặc phải tích hợp abstraction `versionToken` của plan 08 và atomic writer của plan 11.

## Mục tiêu

1. Sửa nhiều vùng không chồng lấn trong một transaction logic, all-or-nothing.
2. `expectedVersion` bắt buộc khi sửa file tồn tại, trừ mode explicit unsafe/force được policy riêng cho phép.
3. Không giữ đồng thời full old file + full new file trong RAM.
4. Preserve encoding/BOM/newline/permissions theo policy.
5. Trước commit phải revalidate version/file identity để bắt concurrent writer.
6. Dry-run trả validation/diff summary nhưng không thay file.
7. Output bounded: summary + preview nhỏ hoặc artifact reference, không full before/after.
8. Cancellation trước commit không thay target; cancellation sau điểm commit có state rõ.

## Contract đề xuất

```json
{
  "path": "src/runtime_host/dispatch.rs",
  "expectedVersion": "v1:...",
  "coordinateSystem": "lineColumn",
  "columnEncoding": "utf8CodePoint",
  "edits": [
    {
      "start": { "line": 10, "column": 1 },
      "end": { "line": 12, "column": 1 },
      "text": "replacement\n"
    }
  ],
  "dryRun": false,
  "preserveLineEndings": true,
  "preserveBom": true,
  "budget": {
    "timeoutMs": 15000,
    "maxBytesRead": 1073741824,
    "maxBytesWritten": 1073741824,
    "maxEdits": 1000
  }
}
```

Nên hỗ trợ canonical coordinate system bằng byte offsets để implementation chính xác và streaming:

```json
{
  "coordinateSystem": "byte",
  "edits": [{ "startByte": 100, "endByte": 120, "text": "..." }]
}
```

Nếu hỗ trợ line/column, phải định nghĩa rõ:

- line 1-based hay 0-based;
- column tính theo byte, Unicode scalar, UTF-16 code unit hay grapheme;
- end exclusive;
- cách xử lý CRLF.

Khuyến nghị line 1-based, column 1-based theo Unicode scalar, end-exclusive cho LLM; server chuyển sang byte ranges trên đúng `expectedVersion`.

Result:

```json
{
  "applied": true,
  "dryRun": false,
  "oldVersion": "v1:...",
  "newVersion": "v1:...",
  "editsApplied": 1,
  "bytesRead": 123456,
  "bytesWritten": 123460,
  "additions": 3,
  "deletions": 2,
  "preview": "bounded unified diff",
  "diffArtifactRef": null
}
```

## Validation semantics bắt buộc

- `edits` không rỗng, không vượt max.
- Ranges phải hợp lệ trong version đã đọc.
- Sort theo start; reject overlap. Adjacent edits được phép.
- Duplicate edits/idempotency phải có behavior rõ.
- Replacement text phải đúng encoding policy.
- Byte boundaries không được nằm giữa UTF-8 code point khi text mode.
- `expectedVersion` mismatch phải fail trước target mutation.
- File đổi sau initial validation nhưng trước commit phải fail ở revalidation cuối.

## Thuật toán streaming đề xuất

1. Resolve path capability và capture version/identity.
2. Verify `expectedVersion`.
3. Convert line/column edits sang byte ranges bằng một streaming pass hoặc line-offset index; không giữ full file.
4. Validate/sort ranges.
5. Tạo temp file cùng directory.
6. Stream source:
   - copy bytes từ current offset đến edit start;
   - ghi replacement;
   - skip source đến edit end;
   - lặp;
   - copy tail.
7. Trong lúc stream tính hash/new version, counters và bounded diff preview.
8. Flush + sync temp theo durability mode.
9. Revalidate target version và parent identity ngay trước commit.
10. Atomic replace target theo plan 11.
11. Capture new version; cleanup temp trên mọi error/cancel.

Không dùng `String::replace` hoặc giữ full `content`/`updated` trong đường chạy mới.

## `fs_replace_text` compatibility

- Giữ tool cũ cho caller hiện tại.
- Với file nhỏ có thể giữ behavior hiện tại trong giai đoạn đầu, nhưng phải thêm cap rõ.
- Hướng tốt hơn: streaming tìm exact occurrences, tạo byte edits rồi gọi cùng transaction engine.
- Nếu file lớn hơn cap và caller dùng legacy tool, trả error hướng dẫn `fs_apply_edits`, không OOM.
- Không tự bỏ `expectedVersion` ở tool mới chỉ vì legacy tool chưa có.

## Các bước triển khai

1. Thêm typed request/result/schema/catalog/dispatch cho `fs_apply_edits`.
2. Tạo module edit engine tách khỏi MCP parsing.
3. Integrate `FileVersion` verify/capture.
4. Implement byte-range engine trước.
5. Implement line/column resolver streaming và CRLF/BOM handling.
6. Integrate atomic writer/durability/permissions.
7. Tạo bounded diff preview; full diff chỉ qua artifact plan 13.
8. Thêm approval summary gồm path, expected version, số edit, estimated bytes; không đưa full content vào approval payload.
9. Cập nhật docs và server instructions để AI ưu tiên `fs_apply_edits` cho file lớn.
10. Thêm legacy adapter/cap cho `fs_replace_text`.

## Test bắt buộc

- Một edit đầu/giữa/cuối file; insert/delete/replace.
- Nhiều edits sorted/unsorted/adjacent/overlap.
- UTF-8 multibyte, emoji, combining characters theo column semantics đã chọn.
- CRLF/LF/mixed và BOM.
- Empty file, no trailing newline, line cực dài.
- File 10 MB/100 MB/1 GB với edit nhỏ ở giữa; memory bounded.
- `expectedVersion` sai, file missing, file replaced concurrently.
- Concurrent writer tại các barrier: trước stream, giữa stream, trước rename.
- Cancellation giữa stream; target không đổi và temp được cleanup.
- Crash/interruption harness quanh flush/sync/rename nếu plan 11 cung cấp fault injection.
- Permissions/read-only behavior.
- Symlink swap/TOCTOU theo plan 07.
- Dry-run không thay mtime/content và trả cùng validation result.
- Idempotent retry behavior theo requestId/expectedVersion.

## Tiêu chí nghiệm thu

- Có native `fs_apply_edits` trong source, schema, catalog, dispatch và packaged smoke test.
- `expectedVersion` được kiểm tra ít nhất hai thời điểm: trước xử lý và ngay trước commit.
- Engine không load full source/new file vào RAM.
- Overlap/range/UTF-8 errors có code riêng, actionable.
- Atomic all-or-nothing trong guarantees của OS; fallback được tài liệu hóa.
- Diff output bounded và không persist full file.
- Legacy `fs_replace_text` có cap/migration rõ.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

Chạy benchmark/stress file lớn và báo peak memory/bytes read/write.

## Kết quả AI phải trả về

- Contract chính xác, coordinate semantics.
- Edit engine và commit flow.
- File đã đổi.
- Concurrency/atomicity guarantees.
- Legacy `fs_replace_text` behavior.
- Test/benchmark và số liệu.

## KIỂM TRA — kết quả hoàn thiện ngày 2026-09-04

Các khoảng trống có thể thực hiện trên máy macOS hiện tại đã được bổ sung và xác minh:

- Thêm deterministic test seam nội bộ cho `fs_apply_edits` tại `BeforeStream`, `MidStream`, `AfterFlush`, `AfterFileSync`, `BeforeVersionRecheck`, `AfterAtomicReplace` và `BeforeDirectorySync`. Test seam chỉ tồn tại dưới `cfg(test)`; contract MCP/public API không thay đổi.
- `deterministic_conflict_barriers_reject_external_writer` PASS tại ba barrier trước stream, giữa stream và trước revalidation/commit; external writer luôn thắng và edit transaction không ghi đè dữ liệu mới.
- `mid_stream_cancellation_keeps_target_and_cleans_temp` PASS; cancellation giữa stream trả `operationCancelled`, target giữ nguyên và temp được cleanup.
- `pre_commit_flush_and_sync_faults_leave_original_and_clean_temp` PASS cho fault sau flush và sau file sync; target cũ không bị thay và staging được dọn.
- `cancellation_after_atomic_replace_stays_committed` PASS; cancellation sau commit point không đổi một mutation đã publish thành lỗi mơ hồ, result vẫn báo `commitState=committed`.
- `readonly_mode_is_preserved_and_symlink_swap_is_rejected` PASS trên macOS: mode `0444` được bảo toàn qua atomic replace; symlink swap ngay trước version recheck bị fail-closed bằng `symlink_traversal_rejected` và victim không bị sửa.
- Approval summary của `fs_apply_edits` đã có `expectedVersion`, `editCount`, tổng replacement bytes ước tính và `contentRedacted=true`; test riêng xác nhận replacement payload không xuất hiện trong summary.
- Thêm benchmark ignored `apply_edits_100mib_and_1gib_reports_peak_rss_and_throughput` và đã chạy thủ công trên máy này:
  - 100 MiB: 5.260 s, 57.03 MiB/s theo tổng bytes read+write, peak RSS 5.44 MiB, RSS growth 0.55 MiB.
  - 1 GiB: 53.707 s, 57.20 MiB/s, peak RSS 5.59 MiB, RSS growth 0.12 MiB.
  - Cả hai trường hợp PASS ngưỡng RSS growth < 128 MiB và giữ nguyên kích thước file.
- Validation PASS: `cargo fmt --check`, `cargo check --workspace`, `cargo test -p chatcmd-runtime`, `cargo test -p chatcmd-mcp`, `cargo test --workspace` và packaged catalog smoke.
- `cargo clippy -p chatcmd-runtime --lib -- -D warnings` vẫn bị chặn bởi 5 lint nền ngoài Plan 09 trong `filesystem_find.rs`, `filesystem_read.rs`, `filesystem.rs`, `process_runner.rs`. Khi allow đúng bốn nhóm lint nền (`collapsible_if`, `too_many_arguments`, `redundant_guards`, `manual_clamp`), runtime lib Clippy PASS; không có lint mới từ phần `fs_apply_edits` vừa bổ sung.

## CÔNG VIỆC CHƯA HOÀN THIỆN — phụ thuộc môi trường Windows

- Chưa thể chạy E2E permission/read-only và symlink-swap của `fs_apply_edits` trên Windows trong lượt này vì máy thực thi hiện tại là macOS và chỉ cài hai Rust targets `aarch64-apple-darwin`, `x86_64-apple-darwin`; không có Windows runtime/runner để xác nhận hành vi thực tế. Phần macOS tương ứng đã PASS.

Do vẫn còn đúng một xác minh phụ thuộc Windows chưa thể thực hiện từ môi trường hiện tại, Plan 09 tiếp tục giữ trạng thái `checked` thay vì `Checkdone` theo quy tắc rà soát.
