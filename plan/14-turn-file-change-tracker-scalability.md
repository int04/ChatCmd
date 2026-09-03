# Plan 14 — Tối ưu TurnFileChangeTracker cho monorepo và diff lớn

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế lại cơ chế theo dõi “Các file đã thay đổi” theo turn. Native filesystem tools phải phát change record trực tiếp; recursive OS watcher chỉ dùng như fallback cho shell/external process và phải có debounce, quota, overflow recovery, ignore policy chung và snapshot bounded. Không commit.

## Ưu tiên

**P1 — hiệu năng, độ chính xác và dung lượng.** Hiện mỗi turn có thể mở watcher recursive trên toàn project; build/restore trong monorepo dễ tạo event storm. Snapshot giới hạn 200 KiB nhưng vẫn đọc toàn file trước khi truncate.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/turn_file_changes.rs:35-76` tạo `RecommendedWatcher` và gọi `watch(&root, RecursiveMode::Recursive)` cho mỗi turn.
- `turn_file_changes.rs:78-112` nhận `__chatcmdDiff` chứa before/after từ output tool.
- `turn_file_changes.rs:114-167` kết thúc turn, đọc current snapshot và đưa full bounded before/after vào result.
- `turn_file_changes.rs:187-239` watcher callback giữ mutex và xử lý từng event/path.
- `turn_file_changes.rs:244-312` hard-code ignored components riêng.
- `turn_file_changes.rs:345-359` `read_text_snapshot` gọi `std::fs::read(path)` toàn file, decode toàn `String`, sau đó mới truncate 200.000 bytes.
- `src/runtime_host/filesystem_dispatch.rs:108-177` native write/replace/delete đã tạo `__chatcmdDiff`, nhưng bằng full snapshot; đây là điểm có thể thay bằng typed explicit change.

Plan này cần phối hợp với plan 07 ignore/path safety, plan 11 atomic writer, plan 13 artifact/persistence và plan 22 shell event lifecycle.

## Mục tiêu

1. Native `fs_write_text`, `fs_apply_edits`, `fs_copy`, `fs_move`, `fs_delete`, `fs_create_directory` phát `FileChangeRecord` chính xác tại commit; không cần watcher phát hiện lại.
2. Chỉ bật recursive watcher khi turn có shell/external process có thể thay file; watcher lifecycle gắn với activity/session, không mặc định mọi turn.
3. Watcher event được debounce/coalesce, bounded theo files/events/bytes/time và không giữ global mutex lâu.
4. Snapshot không đọc toàn file khi chỉ cần preview; stat size trước, stream prefix/suffix/range quanh edit hoặc dùng artifact/diff engine.
5. File lớn/binary trả summary/hash/version + artifact ref, không full before/after.
6. Event overflow/loss được phát hiện và reconcile có budget; không im lặng cho diff sai.
7. File create rồi delete trong cùng turn tiếp tục bị ẩn đúng; rename/move được nhận diện tốt hơn delete+add khi có identity.
8. Kết quả deterministic, dedupe theo canonical path/file identity và không ghi nhận temp files của atomic writer.

## Thiết kế typed record

Tạo type chung thay `__chatcmdDiff` ad hoc:

```rust
FileChangeRecord {
    path: PathBuf,
    previous_path: Option<PathBuf>,
    kind: FileChangeKind,
    origin: ChangeOrigin,
    old_version: Option<String>,
    new_version: Option<String>,
    old_size: Option<u64>,
    new_size: Option<u64>,
    additions: Option<u64>,
    deletions: Option<u64>,
    preview: Option<DiffPreview>,
    diff_artifact_ref: Option<String>,
    confidence: ChangeConfidence,
}
```

`origin`: `nativeTool`, `shellWatcher`, `externalWatcher`, `reconciled`.

`confidence`: `exact`, `sampled`, `metadataOnly`, `unknownDueToOverflow`.

Native tool commit trả record riêng cho tracker, không cần nhét private `__chatcmdDiff` vào public MCP output. Public result có summary cần thiết theo plan 02.

## Native tool integration

- Atomic writer biết old/new version, size, path và có thể tạo bounded diff preview trong lúc stream.
- Range edit biết chính xác ranges/additions/deletions.
- Copy/move/delete journal biết danh sách/counters; nếu quá nhiều file, emit aggregate + artifact manifest thay vì một record mỗi file trong RAM.
- Temp/staging/quarantine path được đánh dấu internal và không xuất hiện trong user-facing changes.
- Record chỉ được publish sau commit; failure/cancel trước commit không ghi modified target.

## Watcher fallback

### Lifecycle

- Bật watcher lazy trước khi `shell_create`/external tool bắt đầu thay đổi workspace.
- Một watcher có thể dùng chung theo workspace root với subscriber theo active turn thay vì một OS watcher mỗi turn, nếu ownership và event routing được thiết kế an toàn.
- Dừng/unsubscribe khi không còn relevant activity; cleanup khi turn/task/app kết thúc.

### Debounce/coalesce

- Callback chỉ đẩy raw event nhỏ vào bounded channel, không đọc file hoặc giữ tracker mutex lâu.
- Worker debounce theo path khoảng 50–250 ms configurable.
- Coalesce create/modify/remove/rename state machine.
- Rate-limit progress/UI updates; final aggregation có cap.

### Overflow

- Detect backend overflow/rescan-needed event.
- Mark tracker degraded và chạy bounded reconcile dựa trên baseline manifest/version index nếu có.
- Nếu không thể reconcile trong budget, trả warning `fileChangeTrackingIncomplete=true` và confidence thấp, không giả vờ đầy đủ.

## Snapshot/diff strategy

Không gọi `std::fs::read` toàn file rồi truncate. Thay bằng:

1. `stat` trước.
2. File nhỏ dưới threshold: đọc bounded toàn file.
3. File lớn:
   - metadata/hash/version;
   - prefix/suffix sample;
   - range quanh native edit nếu biết;
   - hoặc tạo unified diff artifact streaming/bounded.
4. Binary: không decode; trả size/hash/version và binary flag.
5. Invalid UTF-8: metadata-only hoặc binary behavior rõ.

Line delta hiện tại so prefix/suffix của snapshot bị cắt có thể sai. Với native edit, dùng edit metadata; với watcher unknown, dùng diff engine có budget hoặc `additions/deletions=null` kèm confidence.

## Ignore/temp policy

- Dùng một `WorkspaceIgnorePolicy` từ plan 07.
- Tách “không traverse mặc định” khỏi “không bao giờ hiển thị”: nếu user trực tiếp sửa file trong `build`/`target`, semantics phải được quyết định rõ.
- Internal temp names không chỉ dựa vào pattern `.tmpXXXXXX`; atomic writer đăng ký exact internal paths/operation IDs để tracker bỏ qua an toàn.
- Không bỏ nhầm file người dùng chỉ vì tên giống tempfile.

## Các bước triển khai

1. Viết test baseline cho behavior UI hiện tại: added/modified/deleted, create-then-delete, temp ignore.
2. Tạo `FileChangeRecord` và tracker API `record_committed_change`.
3. Migrate native filesystem mutations sang explicit records; loại `__chatcmdDiff` dần.
4. Thay snapshot helper bằng bounded stat/read/diff service.
5. Thiết kế watcher manager lazy/shared với bounded channel.
6. Implement debounce/coalesce/overflow state.
7. Dùng shared ignore policy và exact internal temp registry.
8. Tích hợp artifact diff/manifest cho large/many-file changes.
9. Sửa persistence/UI schema có version; lazy fetch diff artifact.
10. Thêm startup/task cleanup và diagnostics counters.

## Edge cases bắt buộc

- Build tạo hàng chục nghìn event/giây.
- File save kiểu temp + rename của editor.
- Atomic writer temp + replace.
- Rename trong/cross directory, case-only rename.
- Create rồi delete; delete rồi recreate; repeated modifies.
- Binary/invalid UTF-8/file >1 GB.
- File bị khóa hoặc permission denied khi snapshot.
- Watcher overflow, dropped channel events, app sleep/resume.
- Hai turns/tasks cùng workspace đồng thời.
- Shell process còn chạy khi parent turn kết thúc/stopped.
- Ignored directory được user thao tác trực tiếp.

## Test bắt buộc

- Native tool record exact và không phụ thuộc watcher timing.
- Watcher chỉ bật khi shell/external activity cần.
- Event storm bounded: channel size, memory, UI event count.
- Debounce state machine cho create/modify/remove/rename.
- Overflow tạo warning/reconcile, không silent success.
- Snapshot file lớn không đọc toàn file; instrument bytes read.
- Binary/invalid UTF-8 metadata-only behavior.
- Temp internal không hiện; user file tên tương tự vẫn hiện.
- Concurrent turns không trộn changes.
- Tracker cleanup sau cancel/crash/restart.
- Frontend tests cho exact/sampled/incomplete/diff artifact states.

## Benchmark bắt buộc

- Workspace 100.000 file; idle turn không mở watcher hoặc không tăng đáng kể tài nguyên.
- Shell build giả lập 100.000 events; đo CPU, memory, channel drops, final records và UI messages.
- File 1 GB modified bằng range edit; bytes snapshot đọc phải bounded.

## Tiêu chí nghiệm thu

- Native tool changes đi qua typed explicit record, không public full `__chatcmdDiff`.
- Recursive watcher không được tạo mặc định cho mọi turn.
- Callback watcher không đọc file/giữ mutex nặng.
- Snapshot large file bounded và binary-safe.
- Overflow/incomplete tracking được báo rõ.
- UI vẫn hiển thị file changes chính xác, có confidence/artifact khi cần.
- Internal temp/staging không gây file change rác.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Chạy frontend typecheck/test/build nếu sửa timeline/diff UI.

## Kết quả AI phải trả về

- Tracker architecture trước/sau.
- Native record và watcher lifecycle.
- Diff/snapshot thresholds và confidence semantics.
- File/UI/schema đã đổi.
- Event-storm/large-file benchmark.
- Test và các giới hạn platform watcher còn lại.
