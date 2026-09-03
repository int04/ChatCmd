# Plan 02 — Chuẩn hóa result envelope, pagination và truncation cho toàn bộ tool

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế và triển khai một result envelope thống nhất cho các tool có thể trả dữ liệu lớn. Mục tiêu là để LLM biết chính xác kết quả có đầy đủ hay không, tiếp tục ở đâu, giới hạn nào đã chạm và dữ liệu lớn nằm ở artifact/content reference nào. Không triển khai lại thuật toán từng tool ngoài phần tối thiểu để tích hợp contract. Không commit.

## Ưu tiên

**P0 — contract nền tảng.** Hiện mỗi tool trả shape riêng; nhiều tool chỉ có boolean `truncated` hoặc trả thẳng `Vec`. Caller không biết vì sao bị cắt, cursor nào cần dùng, bao nhiêu byte/file đã đọc, hay có artifact chứa phần còn lại không.

## Bằng chứng hiện tại cần đọc lại

- `crates/chatcmd-runtime/src/filesystem.rs:45-344`: `list` và `find` trả `Vec`, `read_text_range` trả `TextReadResult`; chưa có page envelope chung.
- `crates/chatcmd-runtime/src/filesystem_search.rs:20-128`: `search` trả `Vec<serde_json::Value>` và dừng ở `max_results` nhưng không trả cursor/truncated reason.
- `crates/chatcmd-runtime/src/services.rs:1-120`: Git trả `CommandOutput` với `stdout`, `stderr`, `truncated`, nhưng không có cursor hoặc artifact reference.
- `src/runtime_host/inputs.rs:140-245` và `crates/chatcmd-mcp/src/lib.rs:250-360`: schema dùng `offset`, `limit`, `maxResults`, `maxCharacters`, chưa có contract page/budget chung.
- `src/runtime_host/persistence.rs:14-135`: output được clone vào timeline rồi mới enrich; metadata correlation hiện được thêm ở tầng khác.

## Mục tiêu

1. Có kiểu dữ liệu dùng chung cho kết quả lớn, deterministic và dễ hiểu cho LLM.
2. Phân biệt rõ:
   - kết quả đầy đủ;
   - bị cắt vì output cap;
   - hết time budget;
   - hết file/byte budget;
   - bị cancellation;
   - còn trang tiếp theo;
   - full data đã chuyển thành artifact/content reference.
3. Cursor là opaque, có version, có scope và chống dùng nhầm giữa path/query/tool khác nhau.
4. Resource usage và warning được chuẩn hóa nhưng không làm payload phình lớn.
5. Tương thích ngược có chiến lược rõ; không âm thầm đổi shape làm client cũ hỏng.

## Contract đề xuất

Tạo các type typed Rust thay vì dựng `serde_json::Value` rải rác:

```rust
ToolResultEnvelope<T> {
    schema_version: u16,
    data: T,
    page: Option<PageInfo>,
    truncation: Option<TruncationInfo>,
    usage: Option<ToolUsage>,
    warnings: Vec<ToolWarning>,
    content_ref: Option<ContentRef>,
}

PageInfo {
    next_cursor: Option<String>,
    has_more: bool,
}

TruncationInfo {
    truncated: bool,
    reason: Option<TruncationReason>,
    returned_items: u64,
    omitted_items: Option<u64>,
}

ToolUsage {
    elapsed_ms: u64,
    files_scanned: Option<u64>,
    bytes_read: Option<u64>,
    bytes_written: Option<u64>,
    output_bytes: u64,
}
```

`TruncationReason` nên là enum serde ổn định, ví dụ `outputLimit`, `itemLimit`, `timeBudget`, `fileBudget`, `byteBudget`, `replayEvicted`, `binaryContent`, `contentExternalized`.

Không bắt buộc mọi tool có mọi field. Field optional phải dùng `skip_serializing_if` để payload nhỏ.

## Quyết định tương thích phải làm rõ trong code

Chọn một trong hai chiến lược và ghi ADR ngắn:

- **Khuyến nghị:** thêm tool/version mới cho các contract thay đổi lớn, ví dụ `fs_search_v2`, trong khi giữ tool cũ một thời gian; hoặc
- Giữ tên tool và thêm field mới mà vẫn giữ các field top-level cũ trong giai đoạn chuyển tiếp.

Không được đổi `Vec` thành object ở tool cũ mà không có migration/test cho extension/UI/caller.

## Các bước triển khai

1. **Lập inventory output hiện tại**
   - Filesystem, Git, shell replay, process list, task/artifact list, skills.
   - Đánh dấu tool có khả năng output lớn và tool nhỏ không cần envelope đầy đủ.

2. **Tạo shared contract module**
   - Đặt ở crate phù hợp để runtime và MCP cùng dùng.
   - Dùng enum/newtype thay cho stringly typed reason.
   - Thêm constructors như `complete`, `paged`, `truncated`, `externalized` để tránh tạo object sai.

3. **Thiết kế cursor codec chung**
   - Cursor chứa version, tool kind, normalized scope hash, state cần resume và expiration nếu stateful.
   - Encode URL-safe Base64; ký HMAC hoặc dùng server-side cursor ID nếu cursor có dữ liệu không nên tin từ client.
   - Trả lỗi riêng `invalid_cursor`, `cursor_scope_mismatch`, `cursor_expired`, `cursor_version_unsupported`.

4. **Tích hợp thử trên một tool nhỏ**
   - Dùng `fs_list_v2` hoặc test-only adapter để chứng minh schema.
   - Không triển khai thuật toán pagination sâu nếu thuộc plan 04; chỉ dựng contract và adapter.

5. **Chuẩn hóa error/result interaction**
   - Cancellation trước khi có dữ liệu có thể là structured error.
   - Nếu đã có partial data hợp lệ, envelope được phép trả partial với `truncation.reason=cancelled` chỉ khi semantics được tài liệu hóa; tránh vừa success vừa error mơ hồ.

6. **Schema MCP và docs**
   - Schema phải mô tả field, default và ví dụ cursor continuation.
   - Cập nhật `docs/mcp_method.md`.
   - Catalog metadata từ plan 01 nên công bố result schema version/capabilities.

7. **Frontend/timeline**
   - UI hiển thị “còn dữ liệu”, “bị cắt do giới hạn”, “xem artifact” mà không render raw metadata khó hiểu.
   - Không persist toàn envelope nếu chứa content lớn; phối hợp plan 13.

## Test bắt buộc

- Serialize/deserialize ổn định; snapshot schema JSON.
- Field optional rỗng không được serialize thừa.
- Mỗi `TruncationReason` có camelCase đúng.
- Cursor cùng query/path decode thành công; đổi path/query/tool phải bị từ chối.
- Cursor malformed, quá version hoặc hết hạn trả error code đúng.
- Backward compatibility test cho result shape cũ trong thời gian migration.
- Envelope không chứa secret, absolute internal temp path hoặc raw cursor signing material.
- `outputBytes` phản ánh kích thước payload thực tế trong sai số được định nghĩa.

## Tiêu chí nghiệm thu

- Có shared typed result contract, không tạo JSON ad hoc ở từng tool mới.
- LLM nhìn một result biết được `hasMore`, `nextCursor`, lý do truncation và usage cơ bản.
- Có chiến lược migration không làm hỏng caller cũ.
- Cursor không thể dùng lại cho query/path khác.
- Tool nhỏ không bị ép mang metadata dư thừa đáng kể.
- Docs có ít nhất ví dụ complete, paged, budget-truncated và artifact-backed result.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

Nếu có frontend type sinh từ schema, chạy typecheck/test liên quan và báo kết quả.

## Kết quả AI phải trả về

- File/type mới và nơi được dùng.
- Contract JSON cuối cùng.
- Chiến lược tương thích đã chọn và lý do.
- Cursor security/scope model.
- Tool mẫu đã migrate.
- Test và command validation.
- Danh sách tool còn cần migrate ở các plan sau.
