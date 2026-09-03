# Plan 01 — Đồng bộ tool catalog giữa source, binary release và connector đang chạy

## Nhiệm vụ dùng cho chat mới

Làm việc trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`. Hãy triển khai cơ chế bảo đảm tool catalog mà source khai báo, schema MCP sinh ra, binary/package release và connector đang kết nối luôn đồng nhất. Không sửa các thuật toán filesystem khác trong plan này. Không commit. Không làm mất thay đổi hiện có.

## Ưu tiên

**P0 — blocker.** Trong source có `fs_replace_text`, nhưng một connector live có thể không discover được tool này. Khi đó AI buộc phải dùng `fs_write_text` để ghi lại toàn file, làm tăng rủi ro và không dùng được exact replacement.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-mcp/src/tool_catalog.rs:1-49` chứa danh sách `TOOL_NAMES`, bao gồm `fs_replace_text`.
- `crates/chatcmd-mcp/src/lib.rs:250-629` khai báo schema và tool methods cho `fs_list`, `fs_search`, `fs_find`, `fs_read_text`, `fs_write_text`, `fs_replace_text`, `fs_write_raw`.
- `crates/chatcmd-mcp/src/lib_tests.rs:100-190` có test catalog unique và fresh MCP connection advertise đủ tool, nhưng mới chạy in-process.
- Runtime dispatch tương ứng nằm tại `src/runtime_host/dispatch.rs:230-310`.
- Audit thực tế từng thấy connector live chỉ discover nhóm `fs_*` nhưng thiếu `fs_replace_text`, dù source/test có tool.

AI phải đọc lại các file trên và xác nhận line hiện tại trước khi sửa.

## Mục tiêu

1. Mỗi build có một định danh catalog ổn định gồm `protocolVersion`, `catalogVersion`, `catalogHash`, `appVersion` và build identifier.
2. Hash được tính từ canonical manifest chứa tên tool, schema JSON chuẩn hóa và capability flags; không chỉ hash danh sách tên.
3. MCP initialize/info hoặc một metadata endpoint phải công bố các trường này.
4. UI/extension/connector phát hiện catalog đổi và buộc refresh/reconnect thay vì tiếp tục dùng schema cache cũ.
5. CI/release phải chạy smoke test trên binary/package thật, mở MCP transport thật và so sánh catalog được advertise với manifest build.
6. Khi mismatch, lỗi phải rõ ràng và có hướng khôi phục, không im lặng coi tool là không tồn tại.

## Không nằm trong phạm vi

- Không thay implementation của `fs_replace_text`.
- Không thêm range edit; việc đó thuộc plan 09.
- Không sửa thuật toán search/read/write.
- Không đổi permission/approval policy ngoài việc đảm bảo metadata đúng.

## Thiết kế đích đề xuất

Tạo một nguồn chân lý duy nhất, ví dụ module `tool_manifest`, sinh cả:

- `TOOL_NAMES` hoặc iterator catalog.
- Tool schema registry.
- Canonical JSON manifest.
- `catalogHash` dạng SHA-256/BLAKE3 hex.
- Capability metadata như `supportsCursor`, `supportsContentRef`, `mutating`, `streaming`, `deprecatedAliases`.

Ví dụ metadata:

```json
{
  "appVersion": "0.1.0",
  "protocolVersion": 2,
  "catalogVersion": 3,
  "catalogHash": "sha256:...",
  "buildId": "git-or-release-id"
}
```

Canonicalization phải deterministic: sort tool theo tên, sort property key, không phụ thuộc thứ tự `HashMap`, bỏ field mô tả thay đổi không ảnh hưởng contract nếu muốn tránh hash đổi vì wording.

## Các bước triển khai

1. **Khảo sát đường đi catalog hiện tại**
   - Xác định nơi `rmcp` tạo schema từ `tool_methods!`.
   - Xác định extension/UI lưu hoặc cache tool list ở đâu.
   - Xác định endpoint `/info`, `/health`, MCP initialize response hoặc server instructions có thể mang metadata.

2. **Tạo manifest canonical**
   - Tránh duy trì hai danh sách tên riêng biệt.
   - Tool registration và manifest phải được tạo từ cùng macro/data structure.
   - Thêm validation compile/test để mọi dispatcher arm đều có manifest entry và ngược lại.

3. **Tính catalog hash**
   - Serialize schema canonical.
   - Hash toàn bộ contract có ảnh hưởng tới caller.
   - Không dùng timestamp làm một phần hash.

4. **Công bố metadata**
   - Ưu tiên MCP server info/capabilities nếu thư viện hỗ trợ.
   - Nếu cần endpoint bổ sung, endpoint phải local, authenticated phù hợp với kiến trúc hiện tại.
   - Log một dòng structured khi MCP session được tạo: app version, catalog version/hash, transport, agent/session correlation; không log secret.

5. **Client cache invalidation**
   - Khi connector/extension thấy hash khác hash đang giữ, xóa schema cache và reconnect/list tools lại.
   - Khi server nhận request cho tool mà client catalog không có, trả diagnostic chứa hash/version hiện tại.
   - Không tạo vòng reconnect vô hạn; có backoff và giới hạn retry.

6. **Release smoke test**
   - Build binary release hoặc binary test tương đương.
   - Khởi động process trên port tạm.
   - Kết nối qua transport thật, thực hiện initialize/list_tools.
   - So sánh tên và schema digest với manifest trong artifact build.
   - Chạy test ít nhất trên macOS và Windows trong CI nếu pipeline hỗ trợ.

7. **Tài liệu và migration**
   - Cập nhật `docs/mcp_method.md` với version/hash behavior.
   - Nêu rõ connector cũ xử lý server mới thế nào.

## Test bắt buộc

- Manifest có tên unique và sort deterministic.
- Mọi tool trong `TOOL_NAMES` có schema và dispatcher; không có tool thừa ở dispatcher/schema.
- Thay một property schema làm `catalogHash` đổi.
- Thay wording description không làm hash đổi nếu description được xác định là non-contractual.
- Hai fresh process cùng source tạo cùng hash.
- Packaged binary advertise `fs_replace_text` và toàn bộ manifest.
- Connector cache hash cũ tự refresh và sau đó gọi được tool mới.
- Mismatch trả lỗi có `serverCatalogHash`, `clientCatalogHash` hoặc diagnostic tương đương.
- Test không làm lộ token path, agent secret hay conversation scope.

## Tiêu chí nghiệm thu

- Không còn trường hợp unit test thấy tool nhưng binary/connector live không thấy mà không có cảnh báo.
- Có một test chạy qua process/transport thật, không chỉ gọi `McpServer` in-process.
- `fs_replace_text` xuất hiện trong list tools của binary hiện tại.
- Catalog hash/version hiển thị được trong diagnostics.
- Connector đổi catalog tự phục hồi trong số retry hữu hạn.
- Không nhân đôi danh sách tool ở nhiều nơi dễ drift.

## Lệnh validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-mcp
cargo test --workspace
```

Chạy thêm smoke test binary/release mới được tạo trong plan. Nếu cần build output riêng để tránh process đang chạy khóa file, dùng target directory tạm và báo rõ.

## Kết quả AI phải trả về

- Danh sách file đã đổi.
- Nguồn chân lý catalog mới nằm ở đâu.
- Cách tính và công bố `catalogHash`.
- Flow refresh/reconnect phía connector.
- Test process/transport thật và kết quả.
- Các vấn đề tương thích còn lại.
