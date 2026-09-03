# Plan 08 — Mở rộng `fs_stat` với `versionToken`, file identity và metadata phục vụ concurrency

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy nâng cấp `fs_stat` để trả một version token đáng tin cậy cho optimistic concurrency và metadata đủ cho read/edit/write an toàn. Không biến `fs_stat` mặc định thành thao tác hash toàn file đắt đỏ; hỗ trợ nhiều mức strength và budget rõ ràng. Không sửa range edit ngoài điểm tích hợp. Không commit.

## Ưu tiên

**P0 — dependency trực tiếp của plan 09 và 11.** Hiện caller không có cách chứng minh file vẫn là phiên bản đã đọc trước khi ghi.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:129-158` `stat()` chỉ trả `name`, `path`, `entryType`, `size`, `readonly`.
- `crates/chatcmd-runtime/src/types.rs` type `FsEntry` cần đọc lại; hiện chưa có version/hash/mtime/permissions đầy đủ trong output được audit.
- `src/runtime_host/inputs.rs` dùng `PathInput { path }`, không có hash mode/budget.
- `crates/chatcmd-runtime/src/filesystem.rs:209-260` `replace_text` đọc file rồi ghi nhưng không nhận `expectedVersion`.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:30-76` overwrite dựa vào `target.exists()` và không revalidate content version.

## Mục tiêu

1. `fs_stat` trả token opaque dùng được trong `expectedVersion` cho write/edit/delete/move.
2. Token metadata nhanh không cần đọc content toàn file.
3. Caller có thể yêu cầu strong content hash khi thật sự cần và có budget/cancellation.
4. Token phải thay đổi khi file bị replace/truncate/modify; nêu rõ giới hạn của mtime granularity trên từng OS.
5. Trả file identity/kind/size/timestamps/permissions cần thiết mà không follow symlink ngoài policy.
6. Không đưa raw device/inode/path-sensitive data vào token theo cách cho client sửa giả.

## Contract đề xuất

Request:

```json
{
  "path": "src/main.rs",
  "versionStrength": "metadata",
  "hashAlgorithm": null,
  "budget": {
    "timeoutMs": 5000,
    "maxBytesRead": 134217728
  }
}
```

`versionStrength`:

- `metadata`: nhanh, token từ identity + size + high-resolution timestamps + type; phù hợp detect phần lớn thay đổi.
- `sampled`: thêm hash một số block đầu/giữa/cuối; chỉ dùng nếu có lý do.
- `content`: hash toàn content streaming; mạnh nhất nhưng tốn I/O.

Result:

```json
{
  "path": "/canonical/.../src/main.rs",
  "entryType": "file",
  "sizeBytes": 12345,
  "readonly": false,
  "modifiedAtNs": 1780000000000000000,
  "createdAtNs": null,
  "permissions": { "mode": "0644" },
  "versionToken": "v1:...",
  "versionStrength": "metadata",
  "contentHash": null,
  "hashAlgorithm": null,
  "symlink": false
}
```

Token phải opaque và bind với canonical path/scope hoặc file identity theo semantics đã chọn. Không dùng plain JSON unsigned mà caller có thể forge.

## Thiết kế version token

Tạo `FileVersion` typed nội bộ, ví dụ gồm:

- canonical root-relative path hash;
- entry type;
- size;
- modified time độ phân giải cao;
- file identity portable (`dev+ino` Unix, volume+file ID Windows) khi có;
- optional content hash;
- token schema version.

Encode token bằng canonical serialization + HMAC secret local, hoặc giữ server-side token ID có TTL. HMAC giúp stateless nhưng secret phải được quản lý và rotate phù hợp. Token không cần bí mật, nhưng phải chống giả nếu dùng làm precondition security/correctness.

Cần phân biệt:

- `versionMismatch`: target tồn tại nhưng version khác;
- `targetMissing`: file đã bị xóa;
- `targetReplaced`: path hiện trỏ file identity khác;
- `versionUnsupported/Expired`.

## Hashing

- Hash streaming bằng `BufReader`, không `fs::read` toàn file.
- Ưu tiên SHA-256 nếu project đã có `sha2`; BLAKE3 chỉ thêm dependency nếu có lý do đo được.
- Kiểm tra cancellation và byte/time budget mỗi chunk.
- Stat trước/sau hash; nếu metadata đổi trong lúc hash, trả `fileChangedDuringHash`.
- Hash symlink target chỉ khi policy cho và semantics rõ; mặc định stat symlink itself.

## Các bước triển khai

1. Đọc và mở rộng `FsEntry` hoặc tạo `FsStatResultV2`; tránh phá tất cả caller cũ.
2. Tạo module `file_version` chứa capture, encode/decode, compare và error typed.
3. Implement metadata version cho Unix/Windows/macOS với `cfg` nhỏ, facade chung.
4. Implement optional content hash streaming.
5. Integrate path capability từ plan 07.
6. Thêm MCP schema mới/field optional và docs.
7. Expose helper `verify_expected_version(path, token)` để plan 09/11/12 tái sử dụng.
8. Không persist raw token vô hạn nếu chứa signed metadata; timeline chỉ cần token hash/short form khi cần diagnostic.

## Edge cases bắt buộc

- File được sửa nhưng giữ nguyên size.
- Atomic replace tạo inode/file ID mới với cùng content/mtime gần nhau.
- Mtime resolution thấp hoặc bị set ngược.
- File append/truncate trong lúc stat/hash.
- Symlink/broken symlink/junction.
- Directory, FIFO/socket/device.
- File không đọc được nhưng metadata đọc được.
- Sparse file rất lớn.
- Token từ app version cũ.

## Test bắt buộc

- Metadata token ổn định khi file không đổi.
- Token đổi khi content/size/mtime/identity đổi theo guarantees đã công bố.
- Strong content hash đúng với known vectors và stream file lớn.
- Forge/tamper token bị từ chối.
- Token path A không dùng cho path B.
- Hash cancellation/time/byte budget.
- File thay đổi trong lúc hash trả conflict.
- Platform-specific file identity tests.
- Backward compatibility của `fs_stat` cũ.

## Tiêu chí nghiệm thu

- Có một API chung để capture và verify version.
- `fs_stat` mặc định vẫn nhanh, không hash full file.
- Strong mode hash streaming và cancellable.
- Version token được dùng được ngay bởi mutation preconditions mà không parse thủ công.
- Behavior khi metadata không đủ mạnh được ghi rõ, không quảng cáo tuyệt đối.
- Token không forge được và không chứa secret.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- `FileVersion` design và fields theo OS.
- Token encoding/signing/rotation strategy.
- Contract `fs_stat` mới.
- Performance của metadata vs content mode.
- Test và validation result.
- Điểm tích hợp sẵn cho plan 09/11/12.

## Kiểm tra bổ sung cần thực hiện

Phần triển khai và toàn bộ kiểm thử workspace đã chạy thành công trên Windows, bao gồm kiểm thử nhận diện file bằng volume serial + file index. Không thể chạy các kiểm thử riêng cho Unix/macOS trên host Windows hiện tại. Cần chạy lại trên ít nhất một host Unix/macOS để xác nhận fingerprint `device + inode`, metadata thời điểm thay đổi của Unix và permission mode hoạt động đúng:

```bash
cargo test -p chatcmd-runtime filesystem::file_version
cargo test --workspace
```
