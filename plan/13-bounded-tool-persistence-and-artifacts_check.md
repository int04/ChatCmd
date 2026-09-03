# Plan 13 — Giới hạn dữ liệu tool lưu vào SQLite/WebSocket và externalize content lớn thành artifact

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế lại persistence/realtime pipeline của tool call để không clone, serialize, lưu và phát nguyên full input/output/content/Base64/diff lớn. Tạo typed summary/redaction/externalization policy, dùng artifact/content reference cho dữ liệu vượt ngưỡng. Giữ UI timeline hữu ích và tương thích dữ liệu cũ. Không commit.

## Ưu tiên

**P0 — memory, disk và privacy.** Hiện một lần ghi file lớn có thể bị nhân bản qua MCP payload, telemetry, timeline input, runtime output, `__chatcmdDiff`, SQLite và WebSocket.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/persistence.rs:14-69` gọi `append_call_event` với full `arguments` ở trạng thái started và full output ở trạng thái succeeded.
- `src/runtime_host/persistence.rs:72-135`:
  - `payload["input"] = value.clone()`;
  - `payload["output"] = value.clone()`;
  - serialize toàn payload vào `TimelineEvent.payload_json`;
  - publish cùng payload qua realtime.
- `src/runtime_host.rs:196-214` clone arguments cho statistics trước/hoặc sau runtime call; cần đọc lại flow hiện tại.
- `src/runtime_host/filesystem_dispatch.rs:108-177` gắn `__chatcmdDiff` với before/after strings; `write_text` còn chuyển ownership `input.content` vào output diff.
- `src/runtime_host/turn_file_changes.rs:78-167` giữ/đưa before/after vào file change result.
- `crates/chatcmd-mcp/src/request_identity.rs:22-33` đã có thêm các full-body copies ở HTTP boundary.
- Artifact registry/read flow tồn tại tại `src/runtime_host/dispatch.rs:431-500`, nhưng hiện `task_artifact_read` đọc text bounded và artifact model cần audit để tái sử dụng đúng.

Plan này phối hợp với plan 02 result envelope, plan 10 blob/contentRef và plan 14 file change tracker.

## Mục tiêu

1. Timeline mặc định chỉ lưu metadata/summary bounded, không lưu full file content, Base64, secret, terminal flood hoặc giant diff.
2. Realtime payload có cap nhỏ hơn hoặc bằng persistence cap; không serialize payload rồi mới phát hiện quá lớn.
3. Dữ liệu lớn cần giữ được lưu dưới artifact store có quota/retention/integrity/ownership, timeline chỉ chứa reference.
4. Input/output redaction dựa trên tool-aware typed policy, không chỉ cắt JSON string mù.
5. UI vẫn hiển thị path, operation, counters, status, preview nhỏ, additions/deletions, hash/version và link artifact khi có.
6. Database migration/reader xử lý được event cũ có full payload.
7. Có metrics về bytes received, persisted, realtime, externalized và redacted.
8. Không để full content xuất hiện trong logs/statistics/error messages.

## Thiết kế contract nội bộ

Tạo abstraction tương tự:

```rust
ToolEventProjection {
    public_summary: serde_json::Value,
    private_metadata: Option<serde_json::Value>,
    external_payload: Option<ArtifactWriteRequest>,
    redactions: Vec<RedactionKind>,
}

trait ToolEventProjector {
    fn project_started(tool: &str, input: &Value, limits: &EventLimits)
        -> RuntimeResult<ToolEventProjection>;
    fn project_finished(tool: &str, output: &Value, limits: &EventLimits)
        -> RuntimeResult<ToolEventProjection>;
}
```

Không bắt buộc projector nhận `Value` lâu dài; tốt hơn typed tool input/result hoặc registry callback từ tool manifest.

Event summary ví dụ cho write:

```json
{
  "activityId": "...",
  "tool": "fs_write_raw",
  "status": "succeeded",
  "input": {
    "path": "assets/big.bin",
    "source": "contentRef",
    "contentBytes": 1073741824,
    "overwrite": false,
    "expectedVersion": null
  },
  "output": {
    "bytesWritten": 1073741824,
    "newVersion": "v1:...",
    "contentSha256": "..."
  },
  "payloadExternalized": true,
  "artifactRef": "artifact:..."
}
```

Không bao giờ lưu `base64`, `content`, encryption key, bearer token, path token, environment secret hoặc raw conversation scope.

## Event limits

Có config typed, ví dụ:

- max persisted event JSON: 64–256 KiB;
- max realtime event JSON: 64–128 KiB;
- max preview per text field: 8–32 KiB;
- max warnings/errors inline: 20–100;
- max artifact size/quota/TTL;
- max artifact generation per task/tool.

Khi vượt cap:

- Project trước khi serialize.
- Nếu payload có giá trị tải lại: stream vào artifact.
- Nếu không cần giữ: cắt có metadata `truncated=true`, reason và original size.
- Không được fail toàn tool mutation chỉ vì timeline summary quá lớn; persistence summary failure cần degraded-mode rõ và diagnostics.

## Tool-aware projection tối thiểu

- `fs_write_text`: path, byte/char count, overwrite, version; không content.
- `fs_write_raw`: path, decoded byte count/hash; không Base64.
- `fs_apply_edits`: path, number/ranges bounded, replacement byte totals; không full replacement nếu lớn.
- `fs_read_text`: path/range/version/returned byte count; content preview bounded, full result chỉ trong MCP response theo cap hoặc artifact.
- `fs_search`/`fs_find`/`fs_list`: counters/page info, bounded first items; full page không cần duplicate vào timeline.
- `git_diff`/`git_show`: command metadata/stat/preview/artifact ref; không full diff.
- `shell_write`: byte count; redact input by default hoặc chỉ preview khi explicitly safe.
- `shell_read`: sequence range/event count/byte count; terminal content lưu ở terminal chunk store có retention riêng, không duplicate trong timeline.
- Auth/agent calls: preserve necessary metadata but redact secrets/private scope.

## Artifact store requirements

- Artifact ID opaque và owner-scoped theo task/session/turn.
- File path registry server-controlled; no path traversal.
- Stream write/read, hash, size, content type, compression optional.
- Quota per task/user/global, retention/GC và startup orphan reconciliation.
- Immutable sau finalize; atomic publish.
- Authorization khi list/read/download.
- UI chỉ fetch artifact khi user mở, không auto-load giant file.
- Artifact metadata event không chứa secret local path.

Có thể dùng artifact registry hiện tại, nhưng phải audit xem nó đang giả định relative workspace path hay managed store; nếu không phù hợp, tách `ToolPayloadArtifactStore` riêng thay vì ép reuse sai.

## Database/migration

- Cân nhắc thêm columns `payload_size_bytes`, `payload_truncated`, `artifact_id`, `schema_version` hoặc giữ trong metadata JSON có index phù hợp.
- Event cũ vẫn đọc được.
- Có migration/cleanup optional cho giant historical rows; không tự xóa dữ liệu người dùng mà không policy rõ.
- Query timeline phải page theo turn/event và không load giant old payload nếu UI chỉ cần summary; có projection SQL hoặc lazy endpoint.

## Realtime

- Serialize summary một lần thành bytes bounded rồi dùng cho DB/pubsub nếu phù hợp, tránh clone `Value` nhiều lần.
- Broadcast channel phải bounded; slow subscriber không giữ giant payload trong RAM.
- Khi event được externalize, realtime chỉ gửi ref/size/preview.
- Có event version để frontend parse.

## Các bước triển khai

1. Lập inventory field nhạy cảm/lớn cho từng tool trong catalog.
2. Tạo `EventLimits`, redaction types và projector registry.
3. Viết projector cho filesystem/Git/shell trước.
4. Tạo/hoàn thiện artifact store streaming + quota/GC/ownership.
5. Thay `append_call_event` để project trước clone/serialize.
6. Tách result dùng cho caller khỏi result dùng cho timeline; không sửa output gốc chỉ để persist.
7. Sửa `record_tool_diff` nhận typed diff summary/ref thay vì tìm full `__chatcmdDiff` trong output.
8. Thêm lazy artifact endpoints/UI và migration event schema.
9. Thêm metrics/diagnostics và content redaction tests.
10. Cập nhật retention/docs/privacy notes.

## Edge cases bắt buộc

- Input JSON chứa nested field tên `content`, `base64`, `token`, `environment`.
- File content nhỏ/lớn, binary, invalid UTF-8.
- Artifact write thất bại sau tool mutation thành công.
- Database unavailable, WebSocket subscriber chậm/disconnect.
- Event cũ nhiều MB.
- Concurrent artifact GC/read.
- Task delete khi artifact đang tạo.
- Error message vô tình chứa replacement text/command input.
- Hash/path metadata có thể nhạy cảm.

## Test bắt buộc

- Gọi write/read/edit với multi-MB payload; inspect SQLite row không chứa marker nội dung bí mật.
- `payload_json` và realtime bytes không vượt cap đã định trừ overhead nhỏ được test.
- Base64/full content không xuất hiện trong DB, logs, WebSocket event và statistics.
- Artifact ref chỉ đọc được bởi đúng owner/task.
- Artifact streaming, quota, TTL, GC, startup reconcile.
- UI/timeline render event summary cũ và mới.
- Failure externalize không làm báo sai trạng thái mutation; event có degraded warning.
- Terminal output không bị duplicate giữa chunk store và timeline.
- Property/fuzz test projector không bỏ sót known sensitive keys hoặc tạo invalid JSON.

## Benchmark bắt buộc

- Tool input/output 1 MB, 100 MB, 1 GB qua contentRef.
- Đo peak memory, SQLite growth, realtime bytes, artifact bytes và serialization time.
- Chứng minh SQLite/realtime growth gần summary size, không gần content size.

## Tiêu chí nghiệm thu

- `append_call_event` không clone full arbitrary input/output vào payload mặc định.
- Có tool-aware redaction/externalization policy và hard caps.
- Full content/Base64/diff lớn không nằm trong timeline SQLite/WebSocket.
- Artifact store có ownership/quota/retention và lazy read.
- UI vẫn hiển thị đủ summary và trạng thái truncation/reference.
- Test tìm marker bí mật qua DB/log/event không thấy.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Chạy frontend build/typecheck/test nếu sửa UI.

## Kết quả AI phải trả về

- Event projection/redaction policy.
- Threshold/config cuối.
- Artifact store/ownership/retention design.
- File/schema/migration/UI đã đổi.
- Test marker/privacy và benchmark DB/RAM/realtime.
- Compatibility với timeline cũ.

## Cần kiểm tra và hoàn thiện sau

Cơ chế chiếu dữ liệu giới hạn theo schema v2, che thông tin nhạy cảm theo kiểu đệ quy, kiểm thử marker trên SQLite/realtime, chính sách chống lưu trùng dữ liệu terminal và hành vi persistence suy giảm đã được triển khai. Các hạng mục sau của Plan 13 vẫn cần được thực hiện trước khi có thể nghiệm thu đầy đủ:

- Triển khai một `ToolPayloadArtifactStore` chuyên dụng và được quản lý. Artifact registry hiện tại trỏ tới các file trong workspace do người dùng kiểm soát và chưa cung cấp thao tác finalize nguyên tử dành riêng cho payload, quota theo từng owner, TTL/GC, đối soát artifact mồ côi khi khởi động hoặc bảo đảm an toàn khi GC/read chạy đồng thời.
- Bổ sung cơ chế tự động externalize theo luồng cho các kết quả read/Git/diff lớn có thể tải lại. Cơ chế chiếu hiện tại giữ lại `contentRef` nếu đã có; trong các trường hợp khác, nó chỉ che hoặc cắt an toàn dữ liệu timeline quá lớn thay vì tạo artifact mới.
- Bổ sung các field/index cho schema cơ sở dữ liệu và truy vấn summary tải lười cho các dòng timeline lịch sử có kích thước nhiều megabyte. Event schema v2 vẫn tương thích ngược qua JSON, nhưng các dòng cũ chưa được migrate hoặc dọn dẹp.
- Xuất các bộ đếm của cơ chế chiếu sang backend metrics của ứng dụng. Event hiện có bộ đếm số byte nhận được/sau khi chiếu cùng metadata về che dữ liệu/cắt bớt, nhưng chưa có metrics tổng hợp cho dữ liệu đã persist, gửi realtime, externalize và che.
- Chạy và lưu kết quả benchmark bắt buộc với `contentRef` kích thước 1 MB, 100 MB và 1 GB, gồm RAM đỉnh, mức tăng dung lượng SQLite, số byte realtime, số byte artifact và thời gian serialize.
- Kiểm thử các tình huống: tạo artifact thất bại sau khi mutation đã thành công, cơ sở dữ liệu ngừng hoạt động, subscriber WebSocket chậm hoặc mất kết nối, xóa task khi đang tạo artifact và GC/read artifact chạy đồng thời.
- Kiểm tra thủ công giao diện timeline với cả event legacy chứa toàn bộ payload và event đã chiếu theo schema v2. Không có code frontend nào thay đổi và các test Rust trong workspace đã đạt, nhưng chưa chạy kiểm tra render ở cấp trình duyệt.
