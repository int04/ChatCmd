# Plan 10 — Thêm blob/content reference để truyền và ghi file lớn theo chunk

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế và triển khai cơ chế truyền nội dung lớn ngoài JSON arguments, dùng blob/content reference theo chunk. Sau thay đổi, `fs_write_text`, `fs_write_raw`, `fs_apply_edits` và artifact flow phải có thể nhận `contentRef` thay cho toàn bộ content/Base64 inline. Giữ inline mode cho payload nhỏ. Không commit và không làm mất thay đổi hiện có.

## Ưu tiên

**P0 — blocker cho file lớn.** MCP request hiện bị giới hạn 4 MiB; raw binary truyền Base64 còn chịu overhead khoảng 33%, trong khi middleware parse và serialize toàn payload nhiều lần.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-mcp/src/request_identity.rs:7-33` đặt `MAX_MCP_BODY_BYTES = 4 * 1024 * 1024`, đọc full body, parse `serde_json::Value`, rồi serialize lại.
- `src/runtime_host/inputs.rs:223-239` yêu cầu `WriteTextInput.content: String` và `WriteRawInput.base64: String`.
- `crates/chatcmd-mcp/src/lib.rs:330-370` schema MCP cũng bắt caller gửi content/Base64 inline.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:16-28` decode toàn bộ Base64 thành `Vec<u8>` trước khi ghi.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:30-76` clone toàn bộ bytes sang một `Vec` khác trước `spawn_blocking`.
- `src/runtime_host/persistence.rs:14-135` có thể persist lại nguyên input/output vào timeline.

Plan này cần phối hợp với plan 11 về atomic commit, plan 13 về persistence/artifact và plan 17 về request identity/body processing.

## Mục tiêu

1. Không cần nhét file lớn hoặc Base64 lớn vào một JSON-RPC request.
2. Upload theo chunk có resume, idempotency, integrity hash, quota, TTL và ownership theo agent/task/turn.
3. Blob được lưu tạm dưới workspace-managed storage, không trực tiếp dưới path đích trước commit.
4. `contentRef` không thể bị dùng bởi task/agent khác hoặc để đọc path tùy ý.
5. Commit vào file đích phải stream từ blob qua atomic writer, không load toàn blob vào RAM.
6. Blob chưa commit được cleanup khi timeout, cancel, app restart hoặc quota pressure.
7. Inline content nhỏ vẫn dùng được, với cap rõ và schema one-of hợp lệ.

## Tool contract đề xuất

### `blob_begin`

```json
{
  "purpose": "fsWriteRaw",
  "expectedSizeBytes": 1073741824,
  "contentType": "application/octet-stream",
  "expectedSha256": "optional-hex",
  "chunkSizeBytes": 1048576,
  "ttlSeconds": 1800
}
```

Kết quả:

```json
{
  "uploadId": "...",
  "contentRef": "blob:v1:...",
  "chunkSizeBytes": 1048576,
  "expiresAtMs": 0,
  "maxSizeBytes": 1073741824
}
```

### `blob_write_chunk`

```json
{
  "uploadId": "...",
  "offset": 0,
  "dataBase64": "...",
  "chunkSha256": "..."
}
```

Chunk vẫn có thể Base64 vì từng request nhỏ và bounded. Nếu transport hỗ trợ binary frame/HTTP upload endpoint an toàn, ưu tiên binary để bỏ overhead; MCP tool có thể trả signed local upload URL/token một lần dùng, nhưng không được lộ secret dài hạn.

### `blob_status`

Trả size đã nhận, missing ranges, expiry, computed hash state và state `uploading|sealed|consumed|aborted|expired`.

### `blob_commit` hoặc `blob_seal`

```json
{
  "uploadId": "...",
  "finalSizeBytes": 1073741824,
  "sha256": "..."
}
```

Sau seal, blob immutable. Tool mutation nhận:

```json
{
  "path": "assets/big.bin",
  "contentRef": "blob:v1:...",
  "overwrite": false,
  "expectedVersion": null
}
```

### `blob_abort`

Idempotent, cleanup file tạm và metadata.

## Quy tắc ownership và security

- `contentRef` phải opaque, ký hoặc map server-side; không chứa raw filesystem path.
- Bind với authenticated agent, task và mục đích; child task chỉ được kế thừa khi policy nói rõ.
- Mỗi call revalidate owner, expiry, state và size/hash.
- Path blob storage không nằm trong user-controlled workspace tree để tránh watcher/diff ghi nhận nhầm.
- File tạm tạo permission tối thiểu; không executable mặc định.
- Không cho `contentRef` tham chiếu symlink/device/FIFO.
- Không log chunk content, Base64, token hoặc hash secret.
- Token upload HTTP một lần dùng phải short-lived, chống replay và giới hạn Content-Length/offset.

## Quota và lifecycle

Cần cấu hình ít nhất:

- max blob size;
- max bytes đang upload mỗi agent/task;
- max số upload đồng thời;
- max chunk size và min chunk size hợp lý;
- TTL uploading/sealed;
- global disk quota;
- cleanup interval và cleanup khi startup.

Khi quota đầy, trả lỗi structured `blobQuotaExceeded` có retryable và usage hiện tại, không xóa blob active tùy tiện.

State transition phải atomic trong SQLite/metadata store. Blob sealed/consumed không được ghi thêm. Retry chunk cùng offset + cùng hash phải idempotent; cùng offset khác bytes phải conflict.

## Thiết kế lưu trữ

- Stream mỗi chunk vào tempfile/sparse file theo offset hoặc append-only protocol.
- Nếu cho out-of-order chunk, giữ bitmap/range map bounded; nếu chỉ sequential thì contract đơn giản hơn và resume bằng `nextOffset`.
- Khuyến nghị MVP sequential upload để giảm complexity, sau đó mở rộng missing ranges nếu có nhu cầu.
- Hash có thể incremental với sequential mode. Với out-of-order cần hash final streaming khi seal hoặc Merkle/chunk manifest.
- `blob_commit` không copy blob vào RAM; mở reader và truyền vào atomic writer.
- Cleanup phải chịu được crash: metadata orphan và temp orphan được reconcile ở startup.

## Các bước triển khai

1. Viết ADR chọn sequential hay out-of-order upload; khuyến nghị sequential MVP.
2. Tạo typed IDs/state/model và schema SQLite nếu cần.
3. Tạo `BlobStore` abstraction: begin, append/write chunk, status, seal, open_reader, consume, abort, gc.
4. Thêm tool schema/catalog/dispatch; bảo đảm plan 01 packaged smoke test thấy tool mới.
5. Mở rộng `WriteTextInput`/`WriteRawInput` theo one-of:
   - `content`/`base64` inline; hoặc
   - `contentRef`;
   - reject nếu cả hai hoặc không có.
6. Đặt inline cap nhỏ, ví dụ 256 KiB–1 MiB theo envelope thực tế; không chỉ dựa vào HTTP 4 MiB.
7. Tích hợp stream reader với atomic writer plan 11.
8. Tích hợp persistence summary plan 13: chỉ lưu ref, size, hash, không lưu chunks/content.
9. Thêm startup GC, periodic GC và cleanup khi task/call cancel.
10. Cập nhật docs, server instructions và ví dụ flow cho AI.

## Edge cases bắt buộc

- Chunk duplicate giống dữ liệu; duplicate khác dữ liệu.
- Offset sai, gap, overlap, chunk vượt expected/final size.
- Upload không khai expected size.
- Hash chunk/final sai.
- Blob hết hạn giữa upload hoặc giữa write target.
- App crash sau chunk, sau seal, giữa consume, sau target commit trước mark consumed.
- Retry `fs_write_*` với cùng contentRef.
- Hai writers cố consume cùng blob.
- Disk full, permission denied, temp directory missing.
- Cross-task/cross-agent ref misuse.
- Cancellation và task deletion.

## Test bắt buộc

- Upload và ghi text/raw lớn theo nhiều chunk; không load full blob vào RAM.
- Sequential resume từ `nextOffset` sau reconnect.
- Idempotent duplicate chunk.
- Integrity mismatch fail và không đổi target.
- Quota per blob/per task/global.
- TTL/GC và startup orphan cleanup.
- Cross-owner access bị từ chối.
- Concurrent seal/consume/abort race.
- Disk-full/fault injection cleanup.
- Inline mode nhỏ vẫn hoạt động; inline quá cap trả hướng dẫn dùng blob.
- Packaged MCP catalog/schema có đủ blob tools và `contentRef`.

## Benchmark bắt buộc

- 10 MB, 100 MB, 1 GB upload + commit.
- Đo throughput, peak memory, số request/chunk, disk usage tạm và cleanup time.
- Peak memory phải xấp xỉ chunk/buffer size, không xấp xỉ blob size.

## Tiêu chí nghiệm thu

- Có flow begin → chunk → seal → consume/abort hoàn chỉnh.
- `contentRef` không chứa path và được scope/expiry kiểm tra.
- File lớn đi qua streaming reader/writer.
- Retry và crash không tạo target dở hoặc temp leak vô hạn.
- Inline payload có cap rõ và lỗi actionable.
- Timeline/log không chứa chunk/Base64/full content.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- ADR upload protocol.
- Tool contracts và state machine.
- File/schema đã đổi.
- Ownership/quota/GC/integrity flow.
- Tích hợp `fs_write_text`, `fs_write_raw`, `fs_apply_edits`.
- Test/benchmark và số liệu.

## KIỂM TRA — Nội dung cần kiểm tra hoặc hoàn thiện sau

Phần lõi đã được triển khai và các kiểm thử tự động hiện có đã chạy thành công: giao thức tải lên tuần tự,
tiếp tục bằng `nextOffset`, xử lý idempotent cho chunk trùng lặp, kiểm tra conflict/integrity/ownership,
`contentRef` cho ba thao tác thay đổi filesystem, writer dạng stream cho text/raw, giới hạn nội dung inline,
che dữ liệu nhạy cảm khi lưu trữ và packaged MCP catalog.

Các mục sau chưa được nghiệm thu đầy đủ và cần kiểm tra/triển khai tiếp trước khi coi Plan 10 hoàn tất:

- `fs_apply_edits` nhận các chỉnh sửa JSON qua blob nhưng edit engine hiện vẫn tạo toàn bộ mảng chỉnh sửa trong RAM;
  cần chạy benchmark và/hoặc dùng parser dạng stream nếu manifest chỉnh sửa có thể rất lớn.
- Luồng artifact hiện chỉ chấp nhận purpose `artifact`; chưa có tool tạo/đăng ký artifact từ `contentRef`.
- Metadata của blob hiện chỉ tồn tại trong phạm vi process. Khi khởi động, hệ thống dọn các byte mồ côi nhưng không phục hồi phiên tải lên để tiếp tục sau khi khởi động lại; chưa có transaction chuyển trạng thái SQLite như thiết kế đầy đủ.
- Cơ chế TTL GC định kỳ đã được kết nối; chưa có bước dọn dẹp trực tiếp khi task bị xóa và chưa có chính sách loại bỏ dữ liệu khi chịu áp lực quota.
- Chưa chạy mô phỏng lỗi cho tình huống hết dung lượng đĩa, lỗi quyền truy cập, mất thư mục tạm, điều kiện tranh chấp giữa seal/consume/abort, crash tại từng điểm commit, hoặc kiểm tra hai writer cạnh tranh trong kiểm thử tích hợp end-to-end.
- Chưa chạy benchmark bắt buộc với 10 MB, 100 MB và 1 GB để ghi nhận throughput, peak RSS, số request, dung lượng đĩa tạm sử dụng và thời gian dọn dẹp. Mới chỉ chạy các kiểm thử chức năng tự động.
- Schema MCP có các field tùy chọn và runtime áp dụng ràng buộc chính xác một field; cần xác nhận client host hiển thị ràng buộc one-of như mong muốn hoặc bổ sung `oneOf` tùy chỉnh trong JSON Schema.
- Việc thử lại `fs_write_*` sau khi blob đã được consume hiện trả về conflict trạng thái; cần quyết định hợp đồng phát lại theo idempotency key và bổ sung kiểm thử nếu muốn thao tác thử lại trả về kết quả trước đó.
