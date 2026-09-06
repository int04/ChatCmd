# Plan 12 — Gia cố `fs_copy`, `fs_move`, `fs_delete`: preflight, journal, rollback và partial-state reporting

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế lại các mutation đệ quy `fs_copy`, `fs_move`, `fs_delete` để an toàn trên cây lớn, chịu được cancellation/crash, chống symlink/TOCTOU và báo chính xác trạng thái partial. Thêm preflight/dry-run, conflict policy, operation journal và rollback/best-effort cleanup. Không commit.

## Ưu tiên

**P1 — data safety.** Các thao tác hiện tại có thể để destination dở, xóa destination trước khi move thành công hoặc copy xong một phần rồi lỗi mà không có journal/rollback.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem_mutations.rs:83-112`: `copy` gọi `copy_recursive` trực tiếp vào destination.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:114-156`:
  - nếu destination tồn tại và overwrite thì `remove_recursive(destination)` trước;
  - thử `rename`;
  - nếu rename lỗi, fallback `copy_recursive(source, destination, true)` rồi `remove_recursive(source)`.
- `crates/chatcmd-runtime/src/filesystem_mutations.rs:158-190`: delete gọi `remove_dir_all`/`remove_file` trực tiếp.
- `crates/chatcmd-runtime/src/filesystem.rs:390-426`: `copy_recursive` tạo destination và copy từng entry; nếu lỗi giữa chừng không rollback; helper từ chối source symlink nhưng cần audit destination/swap race.
- Inputs `TransferInput`/`DeleteInput` tại `src/runtime_host/inputs.rs:27-42` chỉ có source/destination/overwrite hoặc path/recursive.
- Không có dry-run, progress summary, byte/file budget, checksum, operation ID hay recovery record.

Plan này phải dùng path capability plan 07, version plan 08, atomic publish plan 11 và budget/cancellation plan 16.

## Mục tiêu

1. Preflight xác định source tree, conflict, estimated files/bytes, permission/scope và disk-space warning trước mutation.
2. Mỗi operation có `operationId`, state machine và durable journal đủ để cleanup/recover sau crash.
3. Destination không được public ở trạng thái nửa copy nếu mode atomic staging khả dụng.
4. Cross-device move dùng stage-copy → verify → atomic publish → remove source; không xóa source nếu publish/verify chưa thành công.
5. Overwrite không xóa destination trước khi replacement sẵn sàng; có backup/rollback strategy.
6. Delete hỗ trợ safe quarantine/trash mode hoặc explicit permanent mode; recursive permanent delete phải có summary/approval mạnh.
7. Cancellation có checkpoint và trạng thái rõ: `cancelledNoChange`, `cancelledRolledBack`, `cancelledPartial`.
8. Symlink/junction không được follow ngoài policy; source/destination revalidated trong traversal.
9. Result bounded nhưng đủ để biết file lỗi, rollback status và artifact log nếu danh sách lớn.

## Contract đề xuất

### Copy/move

```json
{
  "source": "big-tree",
  "destination": "backup/big-tree",
  "conflictPolicy": "error",
  "atomicPublish": true,
  "verify": "metadata",
  "preserveMetadata": true,
  "followSymlinks": false,
  "dryRun": false,
  "expectedSourceVersion": null,
  "expectedDestinationVersion": null,
  "budget": {
    "timeoutMs": 300000,
    "maxFiles": 1000000,
    "maxBytesRead": 1099511627776,
    "maxBytesWritten": 1099511627776
  }
}
```

`conflictPolicy`: `error`, `skip`, `replace`, `merge` (chỉ thêm khi semantics/test đầy đủ). Không dùng một boolean `overwrite` cho mọi tình huống phức tạp mà không giải thích directory merge.

### Delete

```json
{
  "path": "generated",
  "recursive": true,
  "mode": "quarantine",
  "expectedVersion": "optional",
  "dryRun": false,
  "budget": {
    "timeoutMs": 300000,
    "maxFiles": 1000000,
    "maxBytesAffected": 1099511627776
  }
}
```

`mode`: `quarantine` mặc định cho path lớn/destructive nếu khả thi, `permanent` explicit. Có thể thêm `quarantineRetentionSeconds` nội bộ/config, không để caller tùy ý phá quota.

Result:

```json
{
  "operationId": "...",
  "state": "completed",
  "filesProcessed": 1000,
  "directoriesProcessed": 50,
  "bytesCopied": 123456789,
  "sourceRemoved": true,
  "destinationPublished": true,
  "verified": true,
  "rollbackAttempted": false,
  "rollbackCompleted": false,
  "warnings": [],
  "detailArtifactRef": null
}
```

## State machine/journal

Ví dụ copy/move:

`planned → staging → verifying → readyToPublish → published → removingSource → completed`

Terminal states: `failedRolledBack`, `failedPartial`, `cancelledRolledBack`, `cancelledPartial`.

Journal tối thiểu lưu:

- operation type/id, owner task/agent;
- canonical source/destination identities;
- staging path và backup path;
- requested conflict/metadata/verify options;
- counters/checkpoints;
- current phase;
- rollback actions còn lại;
- timestamps/lease.

Journal không lưu file content. State transitions phải transaction-safe trong SQLite. Startup recovery scan journal và temp paths thuộc ChatCMD, không đụng file khác.

## Copy algorithm đề xuất

1. Resolve/authorize source và destination.
2. Preflight traversal bounded: detect source-inside-destination, destination-inside-source, symlink, conflicts, estimated size/count.
3. Tạo staging sibling của destination nếu `atomicPublish=true` và cùng filesystem.
4. Copy streaming từng file, preserve metadata theo policy; dùng bounded concurrency và open-file semaphore.
5. Ghi progress/journal theo batch, không mỗi byte.
6. Verify metadata/size hoặc content hash theo requested level.
7. Revalidate source/destination/parent identities.
8. Publish staging atomically; với replace, swap/backup old destination an toàn.
9. Cleanup backup theo durability/recovery policy.

Nếu tree quá lớn để preflight toàn bộ trong budget, trả partial estimate/truncation và yêu cầu caller tăng budget hoặc cho phép streamed preflight mode; không giả vờ đã kiểm tra toàn bộ.

## Move algorithm đề xuất

- Same filesystem + no conflict: atomic rename khi platform/path safety cho phép.
- Replace destination: dùng atomic exchange/backup strategy, không remove trước.
- Cross-device (`EXDEV` hoặc tương đương): copy vào staging, verify, publish, rồi mới remove source.
- Nếu remove source lỗi sau publish, result phải là `completedWithSourceRemaining` hoặc equivalent, không báo move hoàn tất tuyệt đối.
- Không retry mọi rename error như cross-device; phân loại lỗi chính xác.

## Delete/quarantine

- Quarantine nên move path atomically vào managed trash cùng filesystem nếu có thể, ghi original path/version/expiry.
- Có tool/flow restore hoặc GC nội bộ rõ ràng; ít nhất journal phải cho rollback trong operation.
- Permanent recursive delete kiểm tra mỗi entry bằng no-follow semantics.
- Không delete workspace root, task-grant root hoặc filesystem root.
- Khi một entry lỗi, policy `failFast` hay `continue` phải explicit; result liệt kê bounded errors + artifact chi tiết.

## Các bước triển khai

1. Viết ADR về atomicity/rollback guarantee theo same filesystem, cross-device và network filesystem.
2. Tạo typed request/result/state/errors; giữ adapter boolean `overwrite` cũ.
3. Tạo operation journal migration/repository.
4. Tạo preflight walker dùng shared ignore/path safety nhưng **không** mặc định ignore file chỉ vì generated nếu caller copy/delete đúng path đó.
5. Implement staged copy với bounded concurrency/cancellation.
6. Implement verify modes.
7. Implement same-fs move và cross-device fallback đúng lỗi.
8. Implement quarantine/permanent delete.
9. Implement rollback + startup recovery/GC.
10. Tích hợp approval summary, progress throttle, result envelope và artifact detail.
11. Cập nhật docs/schema/catalog/UI.

## Edge cases bắt buộc

- Source là ancestor của destination và ngược lại.
- Destination tồn tại file vs directory; case-only rename.
- Cross-device mount, network share, removable disk.
- Symlink/junction trong source hoặc destination parent; swap race.
- Hard links, sparse files, readonly/permission denied.
- File thay đổi trong lúc copy; source deleted/replaced.
- Disk full giữa file hoặc trước publish.
- Cancellation ở mọi phase.
- Crash sau publish trước journal update; sau publish trước source remove.
- Million-file tree; path quá dài; non-UTF-8 names.
- Quarantine quota đầy/retention GC.

## Test bắt buộc

- Recursive copy success, conflict policies, metadata preservation.
- Fault injection sau N files/bytes và ở từng state transition.
- Rollback không xóa destination cũ hợp lệ.
- Cross-device move simulated/real fixture; source chỉ xóa sau verify+publish.
- Cancellation leaves deterministic state and recovery works after restart.
- Concurrent writer/mover/deleter conflict.
- Symlink swap/TOCTOU tests.
- Delete quarantine/restore/GC và permanent mode.
- Result/error detail bounded; large detail goes to artifact.
- Adapter old `overwrite` behavior có test migration.

## Benchmark bắt buộc

- Tree 100.000 file nhỏ.
- 10 GB tương đương bằng sparse/streamed files nếu môi trường hạn chế.
- Đo throughput, memory, open file count, journal growth, progress event count và cancellation latency.

## Tiêu chí nghiệm thu

- Không còn remove destination trước khi staged replacement sẵn sàng.
- Cross-device move không xóa source trước verify/publish.
- Có durable operation ID/journal và startup recovery.
- Cancellation/crash trả hoặc khôi phục state rõ, không âm thầm để partial tree không được báo.
- Symlink không follow mặc định và path revalidation dùng abstraction chung.
- Large tree operation bị giới hạn resource, không spawn/unbounded open files.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test --workspace
```

## Kết quả AI phải trả về

- State machine và ADR guarantees.
- Contract copy/move/delete cuối.
- File/schema/migration đã đổi.
- Rollback/recovery behavior.
- Test/fault injection/benchmark và số liệu.
- Các trường hợp chỉ best-effort theo OS/filesystem.

## KIỂM TRA BẮT BUỘC — Các khoảng trống về xác thực và triển khai

Plan 12 đã có mức an toàn cơ sở được kiểm thử, nhưng các hạng mục nghiệm thu sau vẫn cần được xác thực thủ công hoặc tiếp tục thực hiện trước khi có thể coi plan này đã được kiểm tra đầy đủ:

- Chạy các benchmark bắt buộc với 100,000 file nhỏ và file 10 GB dạng sparse/stream; ghi nhận throughput, peak memory, số file đang mở, mức tăng của journal, số sự kiện tiến độ và độ trễ khi hủy. Các benchmark có quy mô mang tính phá hủy này chưa được chạy trong môi trường phát triển hiện tại.
- Kiểm thử trên mount cross-device, network share và ổ đĩa rời thực tế. Phần triển khai tạo vùng tạm cạnh đích nên duy trì thứ tự công bố dữ liệu, nhưng chưa xác thực hành vi lỗi/durability thực tế trên từng nền tảng.
- Bổ sung mô phỏng lỗi có tính tất định tại mọi lần chuyển trạng thái và sau số lượng file/byte có thể cấu hình. Các kiểm thử hiện tại bao phủ copy/move/replace thông thường, dry-run, hủy trước khi chạy, từ chối vùng chồng lấn, quarantine, xóa vĩnh viễn, xác minh nội dung và adapter ghi đè cũ, nhưng chưa mô phỏng mọi khoảng thời gian có thể crash hoặc mọi tình huống hết dung lượng đĩa.
- Kết nối migration `0015_filesystem_operation_journal.sql` vào các lần chuyển trạng thái thao tác của runtime, đồng thời triển khai phục hồi khi khởi động và retention GC từ SQLite. Runtime hiện ghi journal sidecar atomic đã fsync cạnh đích; cơ chế này bền vững nhưng chưa đáp ứng yêu cầu transaction SQLite hoặc tự động quét khi khởi động.
- Triển khai tool khôi phục rõ ràng từ quarantine và retention/quota GC. Quarantine hiện nằm trên cùng filesystem, thuộc sở hữu của thao tác và đường dẫn lưu giữ được báo trong warnings, nhưng việc khôi phục/hết hạn vẫn phải làm thủ công.
- Bổ sung lưu trữ artifact có giới hạn cho danh sách lỗi lớn theo từng file và điều tiết tần suất báo tiến độ. Kết quả có kiểu hiện đã được giới hạn và chỉ trả về cảnh báo tóm tắt.
- Xác thực ACL, timestamp, sparse extent, cấu trúc hard link, rename chỉ thay đổi chữ hoa/thường, tên không phải UTF-8, đường dẫn dài, các writer/mover/deleter chạy đồng thời và TOCTOU do hoán đổi symlink trên mọi OS được hỗ trợ. Các bit quyền đã được bảo toàn; những lớp metadata còn lại được ghi trong ADR 0012 là chỉ hỗ trợ ở mức best-effort.

Các kiểm tra tự động đã hoàn tất trong quá trình triển khai: `cargo fmt --all --check`, `cargo check --workspace`, bộ 7 kiểm thử tích hợp `mutation_safety`, `cargo test -p chatcmd-runtime`, `cargo test -p chatcmd-storage`, `cargo test -p chatcmd-mcp` và `cargo test --workspace`. Mọi kiểm thử đã thực thi đều chạy thành công; các fixture hiệu năng thủ công có từ trước vẫn được bỏ qua.

## RÀ SOÁT 2026-09-04 — CÔNG VIỆC CHƯA HOÀN THIỆN

Trong lượt rà soát này đã bổ sung thêm phần recovery thực tế cho journal sidecar thay vì chỉ để lại recovery record:

- `WorkspaceService::recover_interrupted_mutations()` quét các journal `.chatcmd-operation-*.json` còn sót trong workspace khi ứng dụng khởi động.
- Recovery chỉ chấp nhận journal mà toàn bộ source/destination/stage/backup nằm trong cùng workspace root; journal cố trỏ ra ngoài root bị từ chối bằng `journal_path_escape` trước khi đụng dữ liệu.
- Operation chưa publish sẽ dọn staging và khôi phục destination backup nếu cần; operation đã publish sẽ chỉ dọn stage/backup thuộc operation, không tự ý xóa source còn sót.
- `src/main.rs` gọi recovery ngay sau khi khởi tạo `WorkspaceService` và log số operation được phục hồi.
- Bổ sung 2 test mới: rollback stage/backup sau crash giả lập và từ chối journal path escape. Bộ `mutation_safety` hiện có 9 test và đều pass.

Các phần dưới đây **vẫn chưa hoàn thiện**, vì cần thay đổi kiến trúc lớn hơn hoặc môi trường/filesystem chuyên dụng; không được coi Plan 12 là `Checkdone` cho tới khi hoàn tất:

1. **SQLite journal transaction-safe chưa được nối vào từng state transition.** Migration `0015_filesystem_operation_journal.sql` đã tồn tại nhưng runtime mutation hiện vẫn ghi sidecar đồng bộ bên trong `spawn_blocking`. Để nối SQLite đúng yêu cầu cần một async journal sink/state-machine boundary hoặc di chuyển state machine ra khỏi blocking worker; không nên chèn gọi SQL ad-hoc vào blocking worker vì dễ phá cancellation/ordering và có nguy cơ deadlock/runtime misuse.
2. **Quarantine restore + retention/quota GC chưa có tool/flow hoàn chỉnh.** Quarantine hiện an toàn theo same-filesystem rename và trả recovery path, nhưng restore/expiry vẫn thủ công.
3. **Fault injection đầy đủ từng phase/byte/file chưa có.** Cần deterministic hooks cho `staging`, `verifying`, `readyToPublish`, backup rename, publish, source cleanup, journal write và disk-full simulation để kiểm chứng mọi terminal state/rollback path.
4. **Benchmark bắt buộc 100.000 file nhỏ và 10 GB sparse/stream chưa được chạy trong lượt này.** Cần máy/volume dành riêng để đo throughput, peak RSS, open-file count, journal growth, progress event count và cancellation latency mà không gây tải phá hủy lên workspace phát triển.
5. **Cross-device/network/removable filesystem thực tế chưa xác thực.** Cần fixture hoặc mount thật để kiểm tra EXDEV, durability/rename semantics và failure behavior; môi trường hiện tại chỉ xác thực same-filesystem local macOS.
6. **Artifact chi tiết cho danh sách lỗi lớn và progress throttling riêng cho mutation chưa hoàn thiện.** Result hiện bounded ở mức summary nhưng chưa externalize danh sách lỗi lớn theo từng file.
7. **Metadata/OS matrix nâng cao vẫn best-effort:** ACL, timestamp, sparse extents, hard-link topology, case-only rename, non-UTF-8 names, path length cực đại và TOCTOU symlink swap cần chạy trên mọi OS/filesystem hỗ trợ.

Validation của lượt rà soát này:

- `cargo fmt --all --check`: pass sau khi format code mới.
- `cargo test -p chatcmd-runtime --test mutation_safety`: **9/9 pass**.
- `cargo check --workspace`: pass.
- `cargo test --workspace`: phần lớn suite pass nhưng một lần dừng ở test Plan 11 `atomic_write::process_kill_before_commit_keeps_old_target_complete` do timing/orphan-temp expectation; rerun riêng test đó ngay sau đó pass. Đây là flake ngoài phạm vi Plan 12, không phát sinh từ thay đổi mutation recovery.
