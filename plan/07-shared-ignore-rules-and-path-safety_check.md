# Plan 07 — Hợp nhất ignore rules và gia cố workspace/path safety, symlink, TOCTOU

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy tạo một abstraction dùng chung cho workspace traversal/ignore và audit rồi gia cố toàn bộ path authorization của filesystem tools. Mọi absolute/relative path phải được xác thực theo workspace root hoặc task-scoped explicit path grant; không dựa vào việc path là absolute để bỏ qua kiểm tra. Chống symlink swap/TOCTOU cho read và mutation trong phạm vi thực tế của các nền tảng hỗ trợ. Không commit.

## Ưu tiên

**P0 về security/correctness; P1 về ignore consistency.** Đây là nền tảng mà các plan read/search/find/write/copy/delete phải dùng chung.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:291-344`:
  - `existing()` canonicalize path nhưng chỉ gọi `ensure_allowed` khi request ban đầu **không phải absolute**;
  - `creation()` canonicalize parent rồi ghép lại file name;
  - authorization dựa trên path string/canonical path, còn thao tác thực xảy ra sau đó.
- `crates/chatcmd-runtime/src/filesystem_search.rs:31-57` dựng `WalkBuilder` và có danh sách default ignored directory riêng ở `:151-194`.
- `src/runtime_host/turn_file_changes.rs:244-312` có một danh sách ignored component hard-code khác.
- `crates/chatcmd-runtime/src/filesystem.rs:356-389` helper `visit` có logic symlink riêng.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:30-190` resolve/authorize trước, sau đó `exists`, remove, persist, rename/copy/delete qua nhiều syscall; có cửa sổ symlink swap/TOCTOU.
- Task-scoped path grants được xây ở `src/runtime_host/dispatch.rs:55-105`; phải đọc thêm `user_message_path_tests.rs` và code trích path grant trước khi sửa.

## Mục tiêu

1. Mọi filesystem operation đều đi qua một `ResolvedWorkspacePath`/capability đã được authorize; không truyền `PathBuf` chưa xác thực sâu vào implementation.
2. Absolute path chỉ được phép khi nằm trong configured root hoặc đúng task-scoped explicit grant đã được server xác nhận.
3. Read/write/delete/move/copy không follow symlink ngoài policy và giảm tối đa race giữa check và use.
4. Source/destination đều được authorize độc lập với operation phù hợp.
5. Ignore behavior dùng chung cho search/find/watcher/indexer, có `.gitignore`, default excludes và user include/exclude rõ ràng.
6. Windows/macOS/Linux path semantics, case sensitivity, junction/reparse point và Unicode được test.

## Thiết kế type-safe đề xuất

Tạo các newtype/capability nội bộ:

```rust
ExistingWorkspacePath {
    canonical_path: PathBuf,
    root: PathBuf,
    identity: FileIdentity,
    kind: EntryKind,
}

CreationWorkspacePath {
    canonical_parent: PathBuf,
    final_name: OsString,
    root: PathBuf,
    parent_identity: FileIdentity,
}

enum PathAccess {
    Read,
    Create,
    Replace,
    Delete,
    MoveSource,
    MoveDestination,
}
```

Constructor là nơi duy nhất authorize scope/policy. Các service method nhận capability type thay vì raw `&Path` khi khả thi.

`FileIdentity` dùng metadata phù hợp nền tảng để revalidate trước commit: device/inode trên Unix, file ID/volume trên Windows hoặc abstraction portable best-effort. Không dùng identity như security token nếu nền tảng không bảo đảm.

## Path authorization bắt buộc

- Luôn `ensure_allowed` sau canonicalization cho path tồn tại, kể cả input absolute.
- Path grant phải có provenance từ exact user message/task, không chấp nhận caller tự thêm scope qua tool arguments.
- Grant file chỉ cấp đúng file; grant directory cấp subtree; không tự widen lên parent/sibling.
- Với create, canonicalize parent và bảo đảm parent nằm trong scope; final component không được là `.`/`..`, separator hoặc alternate data stream nguy hiểm.
- Source và destination của copy/move phải nằm trong authorized scopes tương ứng.
- Không cho delete/move configured workspace root hoặc grant root nếu policy không cho.

## Chống symlink/TOCTOU

Ưu tiên handle-relative APIs:

- Unix: `openat`/`openat2` với flags như `O_NOFOLLOW`, `RESOLVE_BENEATH` khi khả dụng; fallback từng component có revalidation.
- Windows: mở handle với reparse-point controls, lấy final path/file ID và từ chối junction/symlink không cho phép.
- Không xóa destination rồi mới tạo mà không giữ identity/parent handle.
- Trước atomic replace, revalidate target/parent identity và `expectedVersion` nếu có.
- Recursive traversal dùng `symlink_metadata`, không follow link mặc định; mỗi child phải được kiểm tra trong root.

Nếu phải dùng fallback portable, tài liệu hóa mức bảo vệ và thêm race tests best-effort; không tuyên bố fully race-free khi không đúng.

## Shared ignore policy

Tạo `WorkspaceIgnorePolicy`/`TraversalOptions` chứa:

- respect gitignore;
- hidden policy;
- default generated directories;
- user include/exclude patterns;
- direct-root override semantics;
- symlink policy;
- max depth.

Một nguồn danh sách default ignore duy nhất. Search, find, watcher và indexer phải gọi cùng abstraction. UI/docs hiển thị default và cách override.

## Các bước triển khai

1. Audit đầy đủ path-grant extraction, additional scopes và policy authorization.
2. Viết test chứng minh absolute path ngoài scope hiện bị từ chối; nếu test hiện pass trái mong đợi, xác định đúng lỗ hổng trước sửa.
3. Tạo capability types và resolver chung.
4. Migrate read/stat/list/search/find trước; sau đó mutation source/destination.
5. Implement revalidation/handle-relative helpers theo `cfg(unix)` và `cfg(windows)`; giữ facade portable.
6. Tạo shared ignore module và migrate search/watcher/find từng bước.
7. Loại bỏ danh sách ignore copy-paste sau khi test parity đạt.
8. Thêm structured diagnostics không lộ path ngoài scope quá mức cần thiết.
9. Cập nhật docs về explicit path grant, symlink và platform guarantees.

## Test bắt buộc

### Authorization

- Absolute path trong root: cho phép.
- Absolute path ngoài root không có grant: từ chối.
- Exact granted file: cho phép file đó, từ chối sibling.
- Granted directory: cho phép subtree, từ chối parent/sibling.
- Relative traversal `../`, mixed separators, Unicode normalization, case variants.

### Symlink/reparse

- Symlink trong root trỏ ra ngoài.
- Symlink component giữa path.
- Broken symlink.
- Swap symlink sau authorize trước read/write/delete bằng barrier test.
- Windows junction/reparse point nếu CI hỗ trợ.
- Recursive traversal không loop.

### Ignore

- `.gitignore`, nested ignore, negate rule.
- Default `target`, `node_modules`, `.git` giống nhau giữa search/find/watcher/index.
- `includeIgnored=true` và explicit exclude precedence.
- Direct root là một ignored directory theo semantics đã chọn.

## Tiêu chí nghiệm thu

- Không có nhánh “absolute path thì bỏ qua ensure_allowed”.
- Path authorization được centralize và unit-testable.
- Mutation không dựa duy nhất vào `exists()`/canonical path cũ trước nhiều syscall.
- Shared ignore list chỉ còn một nguồn chân lý.
- Symlink/junction ngoài scope bị từ chối nhất quán.
- Có tài liệu trung thực về guarantees và fallback theo OS.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

Chạy test platform-specific trên macOS hiện tại; bảo đảm CI Windows có test reparse/junction hoặc ít nhất compile/test helper Windows.

## Kết quả AI phải trả về

- Lỗ hổng/behavior path cũ đã xác nhận.
- Capability/resolver design.
- File đã migrate và file chưa migrate.
- Guarantee symlink/TOCTOU theo từng OS.
- Shared ignore precedence.
- Test và validation result.

## CHECK — Platform and race validation still required

The implementation and complete workspace test suite pass on Windows, but the following Plan 07
acceptance items could not be fully exercised in this environment and require follow-up:

- Run the Unix/macOS-only symlink-component and broken-symlink tests on macOS and Linux CI.
- Add and run a Windows junction/reparse-point integration test on a CI runner where creating those
  filesystem objects is permitted.
- Add a barrier-controlled adversarial test that swaps a path component to a symlink between
  authorization and read/write/delete. Current capability identity and parent revalidation are
  best-effort and deliberately do not claim handle-relative, fully race-free behavior.
- Verify case-variant and Unicode-normalization authorization on both case-sensitive and
  case-insensitive macOS filesystems.
- No separate workspace indexer exists in the current tree; if one is introduced, migrate it to
  `WorkspaceIgnorePolicy` and add parity tests with search, find, and the turn watcher.
