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
