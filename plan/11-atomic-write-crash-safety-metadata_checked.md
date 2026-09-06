# Plan 11 — Gia cố `fs_write_text`/`fs_write_raw`: atomic replace, crash safety và bảo toàn metadata

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy viết lại lớp atomic writer dùng chung cho `fs_write_text`, `fs_write_raw`, `fs_apply_edits` và blob consume. Mục tiêu là không có khoảng trống target khi overwrite, không để file dở sau lỗi/crash, hỗ trợ `expectedVersion`, bảo toàn metadata theo policy và có durability mode rõ ràng trên macOS/Linux/Windows. Không commit.

## Ưu tiên

**P0 — correctness/data safety.** Implementation hiện gọi remove target trước khi persist tempfile, vì vậy overwrite không còn atomic và có thể mất target nếu bước sau thất bại.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem_mutations.rs:30-76`:
  - clone `bytes` toàn bộ vào `Vec`;
  - tạo `NamedTempFile` cùng parent;
  - `write_all` + `flush`;
  - nếu overwrite và target tồn tại thì `fs::remove_file(&target_clone)`;
  - sau đó mới `temporary.persist(&target_clone)`.
- Không thấy `sync_data`/`sync_all` cho temp hoặc fsync parent directory.
- Không thấy preservation mode/ACL/xattr/ownership/timestamps.
- `target.exists()` được kiểm tra nhiều lần, có race và không dùng `expectedVersion`.
- `write_raw` decode full Base64 tại `filesystem_mutations.rs:16-28` và `write_bytes` nhận full slice.
- `replace_text` gọi lại writer này tại `filesystem.rs:209-260`.

Plan này nên dùng path capability plan 07, version token plan 08 và stream/contentRef plan 10.

## Mục tiêu

1. Overwrite file thường phải là atomic replacement trong phạm vi guarantee của filesystem/OS; không remove target trước.
2. Create-new phải chống race (`create_new` semantics), không dựa vào `exists()` rồi create.
3. `expectedVersion` được verify ngay trước commit khi overwrite.
4. Input được stream; memory phụ thuộc buffer/chunk, không phụ thuộc file size.
5. Có durability mode được định nghĩa rõ: `none`, `data`, `full`.
6. Bảo toàn hoặc thiết lập permissions/BOM/newline/metadata theo explicit policy.
7. Temp file và journal được cleanup trên mọi error/cancel; startup có orphan cleanup.
8. Result trả commit state, old/new version, bytes written và durability achieved.

## Contract đề xuất

Mở rộng các mutation input dùng chung:

```json
{
  "path": "src/main.rs",
  "contentRef": "blob:v1:...",
  "overwrite": true,
  "expectedVersion": "v1:...",
  "metadataPolicy": "preserve",
  "durability": "data",
  "lineEndingPolicy": "preserve",
  "bomPolicy": "preserve",
  "createParents": false
}
```

`metadataPolicy` tối thiểu:

- `preserve`: giữ mode/ACL/xattr/owner trong khả năng được phép; failure critical hay warning phải định nghĩa.
- `default`: file mới dùng secure/default process umask, không copy metadata.
- `explicit`: caller cung cấp subset được policy cho phép.

`durability`:

- `none`: atomic visibility best-effort, không yêu cầu fsync.
- `data`: sync nội dung temp trước replace.
- `full`: sync temp và parent directory/metadata trong khả năng OS.

Result:

```json
{
  "committed": true,
  "created": false,
  "atomic": true,
  "durabilityRequested": "data",
  "durabilityAchieved": "data",
  "bytesWritten": 1234,
  "oldVersion": "v1:...",
  "newVersion": "v1:...",
  "metadataPreserved": true,
  "warnings": []
}
```

## Thiết kế atomic writer

Tạo abstraction typed, ví dụ:

```rust
AtomicWriteRequest<R: Read> { ... }
AtomicWriteOutcome { ... }
trait AtomicReplaceBackend { ... }
```

Facade chung thực hiện:

1. Resolve/authorize target và canonical parent bằng capability plan 07.
2. Capture target identity/version/metadata nếu tồn tại.
3. Validate create-vs-overwrite và expectedVersion.
4. Tạo temp file **cùng directory** bằng tên khó đoán, flags no-follow/create-new.
5. Set metadata cần thiết vào temp tại thời điểm phù hợp.
6. Stream input vào temp, hash/counter/cancellation theo chunk.
7. Flush và sync theo durability mode.
8. Revalidate parent và target version/identity ngay trước commit.
9. Gọi platform backend atomic replace/create.
10. Sync parent directory nếu `full` và platform hỗ trợ.
11. Capture new version; cleanup state/journal.

Không có bước xóa target trước replace.

## Platform backend

### Unix/macOS

- Dùng rename atomic cùng filesystem; khi cần no-replace cân nhắc `renameat2(RENAME_NOREPLACE)` trên Linux và fallback an toàn.
- macOS có semantics riêng; kiểm tra API phù hợp, không giả định Linux syscall.
- Mở parent handle và dùng handle-relative operation nếu plan 07 triển khai.
- Với durability full, sync file rồi sync directory; tài liệu hóa filesystem có thể vẫn có giới hạn.

### Windows

- Dùng API replace/rename phù hợp (`ReplaceFileW`, `MoveFileExW` hoặc Rust crate an toàn) để thay target không tạo khoảng trống.
- Xử lý sharing violations, antivirus/indexer locks và retry bounded chỉ cho lỗi retryable.
- Bảo toàn ACL/attributes theo policy.
- Không dùng remove-then-rename fallback.

Nếu không thể bảo đảm atomic trên filesystem/network share cụ thể, trả `atomic=false`/warning hoặc fail theo `requireAtomic=true`; không im lặng tuyên bố thành công atomic.

## Text-specific policy

- `fs_write_text` phải validate UTF-8 input.
- Với overwrite và `lineEndingPolicy=preserve`, detect từ bounded sample hoặc metadata read; mixed newline không được normalize ngoài ý muốn.
- `bomPolicy=preserve|add|remove` rõ ràng.
- Không tự thay CRLF/LF chỉ vì caller chạy trên OS khác.
- Permissions executable bit của script phải được giữ khi preserve.

## Metadata preservation

Ít nhất test/implement:

- POSIX mode bits.
- Windows readonly/attributes.
- Modified ownership chỉ khi có quyền; không cố chown tùy tiện.
- ACL/xattr/resource forks: quyết định support/best-effort rõ và warning typed.
- Timestamps: thường mtime mới là đúng sau edit; không nên preserve mtime mặc định vì làm version detection sai.

## Các bước triển khai

1. Tách atomic writer sang module riêng dưới 500 dòng; platform backend tách file `unix.rs`, `windows.rs` nếu cần.
2. Thiết kế typed options/outcome/errors.
3. Implement streaming source (`AsyncRead` hoặc blocking `Read`) và bounded buffer.
4. Implement create-new atomic/no-clobber.
5. Implement replace backend không remove target trước.
6. Tích hợp `expectedVersion` và path revalidation.
7. Tích hợp metadata/durability/text policies.
8. Migrate `fs_write_text`, `fs_write_raw`, edit engine và blob consume.
9. Thêm fault-injection points cho test trước/sau write, sync, revalidate, rename, directory sync.
10. Thêm startup orphan temp cleanup chỉ nhận diện file do ChatCMD tạo, không xóa temp của ứng dụng khác.
11. Cập nhật schema, docs, approval summary và result envelope.

## Test bắt buộc

- Create mới, overwrite, no-overwrite race hai writers.
- Target luôn tồn tại trước hoặc sau failure; không có window remove target.
- Concurrent writer làm `expectedVersion` conflict.
- Fault injection tại mọi phase; target old hoặc new hoàn chỉnh, không partial.
- Process crash harness quanh commit nếu khả thi.
- Disk full/short write/sync failure/rename failure/sharing violation.
- Cancellation giữa stream và ngay trước commit.
- Permissions executable/readonly, CRLF/LF/BOM.
- Symlink swap và parent replacement.
- Cross-device/temp misconfiguration bị phát hiện trước commit.
- Network filesystem/non-atomic backend warning/failure behavior.
- Temp/journal cleanup sau success/error/restart.

## Benchmark bắt buộc

- Stream 10 MB, 100 MB, 1 GB từ blob/file source.
- Đo throughput, peak memory, sync cost theo durability mode.
- Chứng minh peak memory xấp xỉ buffer size.

## Tiêu chí nghiệm thu

- Không còn `remove_file(target)` trước persist/rename trong overwrite path.
- Atomic writer là một abstraction dùng chung, không copy logic ở từng tool.
- Create-new không có check-then-create race.
- Expected version được revalidate ngay trước commit.
- Durability/metadata guarantee được trả trong result và docs.
- Fault-injection tests chứng minh không tạo target dở.
- Không load full content trong writer mới.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

Chạy platform-specific tests trên macOS hiện tại và bảo đảm Windows code có CI compile/test.

## Kết quả AI phải trả về

- Atomic commit algorithm theo OS.
- File/module đã đổi.
- Durability và metadata semantics.
- ExpectedVersion/revalidation flow.
- Fault-injection/crash test result.
- Benchmark throughput/RAM.

## Kiểm tra còn lại

- Chưa chạy kiểm thử/CI trên macOS và Linux; cần xác nhận trực tiếp atomic rename, POSIX mode và đồng bộ thư mục bằng fsync trên hai nền tảng này.
- Chưa có bộ khung kiểm thử crash ở cấp process và mô phỏng lỗi cho từng giai đoạn (ghi thiếu, hết dung lượng đĩa, lỗi đồng bộ, lỗi rename, Windows sharing violation và crash ngay trước/sau commit).
- Chưa benchmark nguồn blob/file 10 MB, 100 MB và 1 GB để ghi nhận throughput, peak memory và chi phí của từng chế độ durability.
- Chưa kiểm thử filesystem mạng/không hỗ trợ atomic để quyết định `requireAtomic`; hiện writer chỉ cam kết thay thế atomic theo primitive cùng thư mục của OS và không dò loại filesystem.
- Metadata `preserve` hiện bảo toàn POSIX mode hoặc quyền chỉ đọc của Windows do `std::fs::Permissions` hỗ trợ; ACL, xattr, owner, resource fork và các thuộc tính Windows khác chưa được bảo toàn hoặc cảnh báo riêng.
- Chưa triển khai chính sách metadata `explicit`, `lineEndingPolicy`, `bomPolicy`, journal và dọn dữ liệu mồ côi khi khởi động. Temp phát sinh khi lỗi/hủy trong cùng process đã được RAII dọn dẹp và có kiểm thử.
- Chưa có kiểm thử điều kiện tranh chấp giữa hai writer cùng tạo file, hoán đổi symlink/thay thế thư mục cha tại thời điểm commit, cấu hình sai cross-device, ghi đè file chỉ đọc trên Windows và BOM/CRLF/LF cho write facade mới.

## Kết quả rà soát bổ sung 2026-09-04

Đã kiểm tra lại implementation hiện tại và bổ sung các phần có thể hoàn tất trực tiếp trên môi trường macOS này:

- Atomic writer dùng chung vẫn commit bằng tempfile cùng thư mục, overwrite không xóa target trước; create dùng no-clobber và expectedVersion được revalidate trước commit.
- Temp atomic writer nay có prefix riêng `.chatcmd-atomic-write-` để crash residue có thể được nhận diện tách biệt với tempfile của ứng dụng khác.
- Đã bổ sung crash harness cấp process chạy chính `write_text_atomic`: child bị kill sau khi temp đã ghi/sync nhưng trước commit; target cũ vẫn nguyên vẹn, không xuất hiện partial target.
- Đã bổ sung test hai writer đồng thời create cùng một path: đúng một writer thắng, writer còn lại nhận conflict/already-exists.
- Đã bổ sung test `fs_write_text` giữ nguyên byte BOM + CRLF; writer không tự normalize newline/BOM của nội dung caller cung cấp.
- macOS POSIX mode + durability `full` tiếp tục pass; `cargo check --workspace --target x86_64-apple-darwin` cũng pass.
- Validation pass: `cargo fmt --check`, `cargo check --workspace`, `cargo test -p chatcmd-runtime`, `cargo test --workspace`.
- Benchmark blob -> atomic writer đã chạy thực tế và pass:
  - 10 MiB: 7.89 MiB/s, peak RSS 12.94 MiB, RSS growth 6.27 MiB.
  - 100 MiB: 7.99 MiB/s, peak RSS 13.08 MiB, RSS growth 0.14 MiB.
  - 1024 MiB: 7.87 MiB/s, peak RSS 14.97 MiB, RSS growth 1.89 MiB.

## Công việc chưa hoàn thiện sau rà soát

Các mục dưới đây vẫn chưa thể xác nhận/hoàn tất đầy đủ trong lượt này, vì cần backend/platform hoặc thiết kế bổ sung vượt khỏi guarantee hiện có; không được coi là đã xong:

- **Startup orphan cleanup an toàn:** crash harness chứng minh process kill để lại một tempfile `.chatcmd-atomic-write-*`. Prefix đã giúp nhận diện residue, nhưng chưa có journal/owner identity/lock/age protocol đủ an toàn để startup quét và xóa mà không có nguy cơ xóa temp của một process ChatCMD khác đang hoạt động. Cần thiết kế ownership/journal rồi mới bật cleanup tự động.
- **Metadata nâng cao:** `preserve` mới bảo toàn `std::fs::Permissions` (POSIX mode hoặc readonly tương ứng). ACL, xattr, owner, macOS resource fork và các Windows attributes khác chưa được copy/cảnh báo typed; `metadataPolicy=explicit` cũng chưa có contract field để caller truyền subset metadata.
- **Text policy đầy đủ:** write facade giữ nguyên chính xác bytes caller gửi, nhưng chưa có `lineEndingPolicy=preserve|...` và `bomPolicy=preserve|add|remove` cho overwrite. `fs_apply_edits` đã có preserveLineEndings/preserveBom riêng, không đồng nghĩa write facade đã có policy này.
- **Fault injection theo từng phase:** đã có process crash thật trước commit và fault tests ở edit engine, nhưng atomic writer chưa có injection riêng cho short-write/disk-full/sync/rename/directory-sync/Windows sharing violation ở từng phase.
- **Network/non-atomic filesystem:** chưa có môi trường network filesystem để xác minh và chưa có filesystem capability detection; `requireAtomic` hiện dựa trên same-directory atomic primitive của backend chứ chưa downgrade/fail theo loại filesystem cụ thể.
- **Linux/Windows validation:** máy hiện chỉ cài `aarch64-apple-darwin` và `x86_64-apple-darwin`; vì vậy chưa chạy Linux/Windows compile/test, Windows readonly/sharing-violation test hay Linux-specific behavior trong lượt này.
- **Benchmark theo từng durability mode:** benchmark 10/100/1024 MiB đã xác nhận streaming/RSS với default `data`; chưa chạy ma trận `none`/`data`/`full` để so riêng sync cost cho từng mode.
