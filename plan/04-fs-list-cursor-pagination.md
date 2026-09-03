# Plan 04 — Nâng cấp `fs_list` thành pagination bằng cursor, không materialize toàn thư mục

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy sửa `fs_list` để làm việc ổn định với thư mục có số lượng entry rất lớn. Thay pagination `offset/limit` hiện tại bằng contract cursor có tính nhất quán, đồng thời tránh đọc, stat và sort toàn bộ directory chỉ để trả một trang nhỏ. Giữ tương thích với tool cũ trong giai đoạn migration. Không commit.

## Ưu tiên

**P1 — quan trọng cho monorepo/generated trees.** Hiện trả 100 entry vẫn có thể phải materialize và sort hàng trăm nghìn entry.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:76-127`:
  - `fs::read_dir` duyệt toàn bộ thư mục;
  - gọi `symlink_metadata` cho từng entry;
  - push tất cả vào `Vec`;
  - sort toàn bộ theo lowercase name;
  - sau cùng mới `drain(offset..).take(limit)`.
- `src/runtime_host/inputs.rs:158-168` dùng `offset: usize`, `limit: usize`.
- `crates/chatcmd-mcp/src/lib.rs:270-305` công bố `offset/limit`, chưa có cursor, `hasMore`, directory version hay truncation reason.

## Mục tiêu

1. Memory bounded theo page size và cấu trúc nhỏ cần thiết, không theo tổng entry trong directory.
2. Có `nextCursor`, `hasMore`, `directoryVersion` và lỗi rõ khi directory thay đổi làm cursor không còn hợp lệ.
3. Không dùng numeric offset làm continuation chính.
4. Cho phép chọn sort/order có semantics rõ; không hứa global alphabetical order nếu implementation không thể đạt bounded memory.
5. Chỉ lấy metadata đắt khi caller yêu cầu.
6. Cancellation/time/item/stat budget áp dụng trong vòng duyệt.

## Quyết định thiết kế bắt buộc

Filesystem API phổ biến không bảo đảm `read_dir` trả thứ tự ổn định. Vì vậy phải chọn và tài liệu hóa một trong các hướng:

### Hướng A — cursor theo snapshot/server-side state

- Lần đầu duyệt/sort tạo snapshot bounded trên disk hoặc cache có TTL.
- Cursor trỏ tới snapshot ID + vị trí.
- Phù hợp khi cần global sort chính xác.
- Phải giới hạn disk/cache, TTL, cleanup và scope security.

### Hướng B — streaming order không bảo đảm global sort

- Trả entry theo filesystem traversal order hoặc order được định nghĩa best-effort.
- Cursor chứa continuation state/last identity.
- Memory thấp hơn nhưng UI/LLM không được hiểu là alphabetically complete.

### Hướng C — index dùng chung

- Dựa vào repository index ở plan 20 để có ordered keyset pagination.
- Chỉ dùng khi index đã sẵn sàng; phải có fallback không index.

Khuyến nghị triển khai A cho UX deterministic hoặc B làm MVP, nhưng không được giữ cách “full sort rồi offset” mà gọi là scalable.

## Contract đề xuất

```json
{
  "path": ".",
  "cursor": null,
  "limit": 200,
  "sort": "nameAsc",
  "metadata": ["type", "size", "readonly"],
  "includeHidden": true,
  "budget": {
    "timeoutMs": 5000,
    "maxEntriesScanned": 10000,
    "maxStats": 1000
  }
}
```

Kết quả:

```json
{
  "items": [
    { "name": "src", "path": "...", "entryType": "directory" }
  ],
  "nextCursor": "...",
  "hasMore": true,
  "directoryVersion": "...",
  "truncated": false,
  "usage": {
    "entriesScanned": 201,
    "metadataCalls": 201,
    "elapsedMs": 3
  }
}
```

`metadata` cho phép caller chỉ yêu cầu `name/path/type` để tránh stat size/permissions khi không cần.

## Các bước triển khai

1. Tách typed input/result cho `fs_list_v2` hoặc contract migration theo plan 02.
2. Tạo `DirectoryVersion` từ canonical directory identity + mtime/change token phù hợp nền tảng.
3. Implement cursor codec scoped theo canonical path, sort mode, filter options và directory version.
4. Implement lựa chọn pagination đã quyết định; ghi ADR giải thích trade-off.
5. Chỉ gọi metadata theo field caller yêu cầu; vẫn dùng `symlink_metadata`, không follow symlink ngoài policy.
6. Dừng traversal khi đủ trang/budget thay vì tiếp tục vô ích.
7. Kiểm tra cancellation token định kỳ trong blocking loop; không chỉ drop future.
8. Trả warning khi một entry biến mất/permission denied trong lúc list; quyết định fail-fast hay skip có thống kê.
9. Giữ adapter `fs_list` cũ với cap an toàn và deprecation metadata.
10. Cập nhật MCP schema, docs và frontend caller nếu có.

## Edge cases bắt buộc

- Directory thay đổi giữa trang 1 và trang 2.
- Tên khác nhau chỉ bởi case trên filesystem case-insensitive/case-sensitive.
- Tên không phải UTF-8; không được làm mất entry hoặc panic vì `to_string_lossy` mà không báo.
- Symlink, broken symlink, FIFO/socket/device entry.
- Entry bị xóa sau `read_dir` trước `metadata`.
- Permission denied cho một entry.
- Directory chứa hàng trăm nghìn file.
- Cursor dùng cho path/sort/filter khác.
- Snapshot/cache hết hạn hoặc app restart.

## Test bắt buộc

- Trang hóa thư mục 0, 1, `limit`, `limit+1`, 10.000+ entry.
- Không trùng/thiếu entry khi directory bất biến qua mọi page.
- Cursor path/sort mismatch bị từ chối.
- Directory version change có behavior đã tài liệu hóa.
- Test memory hoặc invariant không giữ `Vec` toàn directory trong streaming mode.
- `metadata=[]` không thực hiện stat không cần thiết, có counter test/fake filesystem nếu cần.
- Cancellation và time budget dừng traversal.
- Tương thích `offset/limit` cũ trong giới hạn đã định.

## Tiêu chí nghiệm thu

- Đường chạy scalable không collect và sort toàn bộ directory trong RAM trừ khi dùng snapshot có giới hạn/TTL rõ ràng.
- Result có `hasMore` + `nextCursor` + reason khi không thể tiếp tục.
- Cursor opaque và bound với path/options/version.
- Metadata cost theo yêu cầu caller.
- Không follow symlink ngoài workspace scope.
- Có benchmark trên thư mục lớn chứng minh page đầu không tỷ lệ tuyến tính với tổng entry, hoặc nếu snapshot mode cần full scan thì chi phí được externalize/cached và thể hiện rõ.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- Pagination strategy đã chọn và lý do.
- Contract JSON cuối.
- File/module đã đổi.
- Cách xử lý directory mutation và cursor expiry.
- Số liệu benchmark page đầu/trang tiếp theo.
- Test và compatibility behavior.
