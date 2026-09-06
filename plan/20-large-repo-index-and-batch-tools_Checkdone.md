# Plan 20 — Thêm repository index tăng dần và batch stat/read cho monorepo lớn

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế và triển khai lớp repository index tăng dần cùng các batch tools để giảm việc quét lại toàn monorepo và giảm số round-trip MCP. Index chỉ là accelerator; correctness phải có stale detection và fallback trực tiếp. Ưu tiên path/content metadata trước, symbol index để phase sau nếu phạm vi quá lớn. Không commit.

## Ưu tiên

**P2 — tối ưu nâng cao sau khi streaming, cursor, budget và path safety đã ổn.** Không dùng index để che các thuật toán unbounded cơ bản.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem_search.rs:20-128` quét repository từ đầu cho mỗi search.
- `crates/chatcmd-runtime/src/filesystem.rs:262-289` `find` quét tree từ đầu cho mỗi call.
- `filesystem.rs:76-127` list/stat từng entry theo call, không có batch/index.
- `src/runtime_host/turn_file_changes.rs` đã dùng watcher nhưng không có durable repository index/reconciliation và event overflow semantics.
- MCP catalog hiện có single-path `fs_stat`, `fs_read_text`; chưa có `fs_batch_stat`, `fs_batch_read` hoặc index status/control.
- SQLite và storage layer đã tồn tại; cần audit migration/query capacity trước khi quyết định dùng SQLite index hay embedded engine khác.

Plan này phụ thuộc plan 07 path/ignore, plan 08 version token, plan 16 budgets và plan 19 metrics. Search/find v2 phải hoạt động không index trước.

## Mục tiêu

1. Index path/entry metadata theo workspace root để `find`, candidate selection cho `search`, batch stat và UI nhanh hơn.
2. Incremental update theo native mutation records + watcher, nhưng định kỳ/restart có reconcile để không tin watcher tuyệt đối.
3. Mỗi indexed record có source version/file identity/mtime/size và trạng thái `fresh|stale|unknown`.
4. Query trả `indexUsed`, `indexGeneration`, `indexFreshness`, warnings và fallback behavior.
5. Index build/update có budgets, cancellation, bounded concurrency và không làm app lag.
6. Disk/memory quota, schema version, rebuild/migration và root removal cleanup rõ ràng.
7. Batch tools giảm round-trip nhưng mỗi batch vẫn có per-item result, aggregate limits và path authorization.
8. Không index file content/secret mặc định ngoài policy; content index có scope/size/ignore controls.

## Phạm vi phase

### Phase A — path/metadata index bắt buộc

Record gợi ý:

```text
workspace_id
relative_path_bytes / display_path
entry_type
size_bytes
modified_at_ns
file_identity
version_token_metadata
extension_normalized
ignored_state
last_seen_generation
```

Hỗ trợ:

- indexed `fs_find_v2`;
- candidate filtering cho `fs_search_v2`;
- `fs_batch_stat`;
- quick workspace summary/file counts.

### Phase B — text trigram/token index tùy chọn

- Chỉ index text file dưới configurable cap.
- Trigram/inverted index để chọn candidate; kết quả cuối vẫn verify bằng streaming search trên current version.
- Không lưu full source nếu không cần.
- Sensitive/excluded directories không index.

### Phase C — symbol index tùy chọn, có thể tách plan sau

- Tree-sitter/LSP/rust-analyzer integration cho symbol/file outline.
- Không làm trong cùng PR nếu đẩy file >500 dòng hoặc kéo scope quá rộng.

## Tool contract đề xuất

### `fs_batch_stat`

```json
{
  "paths": ["src/a.rs", "src/b.rs"],
  "versionStrength": "metadata",
  "maxItems": 500,
  "budget": { "timeoutMs": 10000, "maxMetadataCalls": 500 }
}
```

Result giữ input order và per-item outcome:

```json
{
  "items": [
    { "path": "src/a.rs", "ok": true, "stat": { "...": "..." } },
    { "path": "src/b.rs", "ok": false, "error": { "code": "notFound" } }
  ],
  "usage": { "requested": 2, "succeeded": 1, "failed": 1 },
  "indexUsed": true,
  "indexGeneration": 42
}
```

### `fs_batch_read`

```json
{
  "requests": [
    { "path": "src/a.rs", "startLine": 1, "lineCount": 100 },
    { "path": "src/b.rs", "startLine": 20, "lineCount": 50 }
  ],
  "maxItems": 50,
  "maxTotalOutputBytes": 1048576,
  "concurrency": 4,
  "budget": { "timeoutMs": 15000, "maxBytesRead": 67108864 }
}
```

Batch read phải gọi streaming reader plan 03, không read full file. Output có per-item truncation/version/cursor; tổng output cap được enforce.

### Index diagnostics/control

Có thể thêm internal/local API hoặc MCP read-only tools:

- `workspace_index_status`;
- `workspace_index_rebuild` cần approval/resource budget;
- `workspace_index_pause/resume` nếu product cần.

Không bắt AI rebuild mỗi query; runtime tự quản lý lifecycle.

## Consistency model

- Mỗi workspace có monotonic `indexGeneration`.
- Initial crawl đánh dấu generation; records không thấy trong crawl complete được remove/tombstone.
- Native mutation commit update index transactionally hoặc enqueue durable update sau commit.
- Watcher updates best-effort và debounce.
- Query index trả candidate với stored version; trước khi trả chính xác hoặc search content, stat/verify current version.
- Stale record được refresh/remove; query có counter `staleEntriesDetected`.
- Watcher overflow/app restart/root changed trigger bounded reconcile/full rebuild.
- Nếu index unavailable/corrupt/migrating, direct fallback dùng plans 04–06 và báo `indexUsed=false`; không fail basic tool.

## Storage design

Đánh giá SQLite hiện tại:

- Tạo tables/indexes tách namespace, không nhét vào timeline.
- Lưu relative path dạng bytes/portable representation để hỗ trợ non-UTF-8 trên Unix; display string riêng.
- Index `(workspace_id, normalized_name/path)`, extension, generation, version fields.
- WAL/busy timeout/batch transaction; không commit từng file.
- Vacuum/retention/rebuild policy.

Nếu dùng engine khác, phải giải thích dependency, binary size, cross-platform, lock/crash recovery và packaging. Ưu tiên SQLite sẵn có nếu benchmark đáp ứng.

## Build/update architecture

- Background indexer dùng weighted admission slot thấp; không cạnh tranh mutation/user call quan trọng.
- Walker dùng shared ignore policy, bounded channel và batch DB writes.
- Cancellation/shutdown clean; checkpoint generation.
- Resume incomplete build hoặc discard/rebuild rõ.
- File content indexing dùng worker count bounded và byte budget.
- Không chạy index trên workspace root chưa được user grant/không còn tồn tại.

## Các bước triển khai

1. Viết ADR consistency/storage/phases và benchmark baseline scan trực tiếp.
2. Tạo workspace identity/index schema/version/migration.
3. Implement initial path/metadata crawl + batch transaction + status.
4. Implement query APIs và direct fallback/stale verification.
5. Hook native file change records; sau đó watcher updates/reconcile.
6. Implement batch stat/read tools với aggregate budgets.
7. Migrate `fs_find_v2` optional index path; search candidate path nếu Phase B.
8. Thêm quota/rebuild/corruption recovery/root cleanup.
9. Thêm diagnostics/metrics/UI status nếu cần.
10. Chỉ sau khi Phase A ổn mới đánh giá text/symbol index bằng benchmark.

## Edge cases bắt buộc

- Workspace 1.000.000 paths.
- App crash giữa crawl/batch commit.
- Rename/move directory lớn.
- Watcher overflow, dropped update, filesystem offline/remounted.
- File changes giữ cùng size/mtime gần nhau.
- Non-UTF-8/case-insensitive/case-only path.
- Two tasks same workspace; multiple roots.
- Ignore config thay đổi.
- Index schema/app version mismatch/corrupt DB.
- Disk quota đầy.
- Symlink/junction/root moved.
- Batch item duplicate và output cap giữa batch.

## Test bắt buộc

- Initial crawl correct; generation/tombstone behavior.
- Incremental create/modify/delete/rename.
- Stale index detection và direct verify/fallback.
- Restart/corruption/migration/rebuild.
- Ignore policy parity với direct tools.
- Query không trả path ngoài scope.
- Batch stat/read giữ order, per-item errors, total budget/cancel.
- Concurrent query + update không panic/deadlock và snapshot semantics rõ.
- Index unavailable vẫn dùng direct path.
- Privacy test không index excluded/secret directories/content.
- Million-path synthetic integration/perf test ở tier phù hợp.

## Benchmark bắt buộc

- Cold initial crawl 100k/1m paths.
- Warm find queries và direct scan comparison.
- Incremental 1/100/10.000 file changes.
- DB size, memory, CPU, query p50/p95, rebuild time.
- Batch stat/read so với N MCP calls.
- Chỉ triển khai text/symbol phase nếu số liệu chứng minh lợi ích.

## Tiêu chí nghiệm thu

- Path/metadata index có generation, stale detection, reconcile và fallback.
- Basic tools vẫn đúng khi index tắt/corrupt.
- Index update không chỉ dựa vào watcher.
- Batch tools có hard caps và streaming reader.
- Query/result công bố index freshness/usage.
- Quota/rebuild/migration/startup recovery được test.
- Không mở rộng sang symbol index nếu Phase A chưa có benchmark xanh.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

## Kết quả AI phải trả về

- ADR/storage schema/indexes.
- Consistency/fallback/reconcile model.
- Tool contracts batch/index status.
- File/migration/UI đã đổi.
- Benchmark cold/warm/incremental/DB size.
- Phase nào hoàn thành, phase nào chủ động để sau.

## Kết quả hoàn thiện Phase A — 2026-09-04

### Architecture

- Repository index là path/metadata index theo từng workspace root, lưu in-memory và snapshot bền vững bằng SQLite.
- Mỗi root có monotonic generation, schema version và freshness `fresh|stale|unknown`; persisted snapshot sau restart luôn được restore ở trạng thái stale cho tới reconcile.
- Unix snapshot giữ raw relative path bytes song song display path; Phase A không lưu source content.
- Hard caps: tối đa 1.000.000 entries và khoảng 512 MiB metadata payload cho snapshot persistence.

### Consistency / lifecycle

- Direct filesystem luôn là source of truth; index chỉ là accelerator.
- Native mutations và watcher events cập nhật/tombstone metadata bị ảnh hưởng theo hướng bounded rồi đánh generation stale.
- Directory delete/rename không quét toàn subtree trong callback; chỉ tombstone exact entry O(1), sau đó direct fallback + periodic reconcile loại descendants.
- Path đã bị delete/rename trên macOS được normalize bằng canonical ancestor còn tồn tại để vẫn map đúng canonical workspace root.
- Watcher object được giữ sống trong runtime host; watcher errors/creation failures mark root stale và persist stale state.
- `start_repository_index_reconcile()` có atomic one-shot guard, tránh spawn duplicate watcher/reconcile khi gọi nhiều lần.
- Periodic reconcile chạy khoảng 60 giây; per-root rebuild mutex serialize rebuild cùng root.
- Corrupt/incompatible/unavailable persisted index không chặn basic filesystem tools; fallback direct vẫn hoạt động.
- Index reconcile task hiện dựa vào Tokio runtime teardown khi app shutdown; chưa có shutdown token riêng cho repository index. Đây là lifecycle limitation đã document trong ADR, không làm sai correctness/fallback Phase A.

### Find / search

- `fs_find_v2` dùng fresh metadata index cho candidate enumeration khi request tương thích; `includeIgnored=true` hoặc custom exclude fallback direct để giữ parity.
- `fs_search_v2` dùng fresh metadata index chỉ để chọn candidate file; content match vẫn đọc live bằng streaming search.
- Indexed candidate được live-verify size, mtime và entry type trước khi return/read.
- Stale/missing candidate mark root stale; first page retry direct, continuation trả typed `cursor_stale`.
- Continuation giờ bind thêm current index generation/freshness: rebuild hoặc generation change giữa hai page trả `cursor_stale` thay vì silently tiếp tục snapshot cũ.
- Ignore/privacy parity và deleted candidate fallback có regression tests.

### Batch tools

- `fs_batch_stat`: hard cap 500 items, giữ input order/per-item error, aggregate metadata/hash byte budget, common wall-clock deadline, cancellation, live exact stat và index diagnostics.
- Mixed-root batch không publish generation mơ hồ (`indexUsed=false`, generation `None`, freshness `Unknown`).
- `fs_batch_read`: hard cap 50 items, bounded concurrency, streaming `fs_read_text_v2`, aggregate read-byte budget, common deadline kể cả semaphore wait, cancellation, ordered results và hard aggregate output cap.

### Storage / recovery

- SQLite snapshot replacement là một transaction; root cleanup, schema mismatch và corrupt-row recovery đều có test.
- Quota dùng entry count + metadata estimate + `PRAGMA page_count`/`page_size` growth accounting trước commit; replacement vượt quota rollback và snapshot cũ vẫn load được.
- Sau successful replacement chạy best-effort `PRAGMA wal_checkpoint(PASSIVE)`; không `VACUUM` mỗi rebuild.
- Quota là logical SQLite growth bound; transient WAL/filesystem overhead có thể khác estimate và được ghi rõ trong ADR.

### Tests Phase A

Target `crates/chatcmd-runtime/tests/repository_index_batch.rs` hiện có 16 tests xanh, bao phủ generation/stale, indexed find/search/stat, parity/direct fallback, aggregate budgets/cancellation, mixed roots, schema mismatch/restart stale, concurrent rebuild serialization, privacy/ignore, deleted candidate, same-size mtime drift, find/search stale cursor và bounded directory rename tombstone.

Storage workspace-index tests bao phủ transactional round-trip/root cleanup, corrupt row recovery và quota reject giữ nguyên snapshot cũ. Non-UTF8 raw filename test chạy trên Unix filesystem hỗ trợ; macOS conditionally skip vì APFS/API có thể reject byte-invalid filename.

### Benchmark

100k synthetic workload đã đo:

- fixture build: ~9,306 ms
- cold rebuild: ~679 ms
- indexed warm find p50/p95: ~163 / 172 ms
- direct late-match p50/p95: ~311 / 313 ms
- incremental 1 / 100 / 10,000 changes: ~0.5 / 10.7 / 1,155 ms
- peak RSS: ~59.8 MB
- SQLite 100k write: ~6,201 ms
- SQLite load p50/p95: ~907 / 951 ms
- SQLite DB/WAL: ~60.3 / 61.4 MB

1,000,000-path benchmark đã chạy thật và PASS:

- fixture build: ~150,505 ms
- cold rebuild: ~67,320 ms
- indexed warm find p50/p95: ~1,662 / 1,669 ms
- direct late-match p50/p95: ~297 / 304 ms
- incremental 1 / 100 / 10,000 changes: ~2.565 / 39.113 / 1,977.936 ms
- batch 500: ~236.190 ms
- sequential 500: ~57.854 ms
- peak RSS: ~752,975,872 bytes
- total benchmark runtime: ~700.23 s

Quan trọng: ở workload 1m này, indexed warm find **chậm hơn direct synthetic late-match**. Không claim index luôn nhanh hơn direct. Đây là observed performance finding của Phase A; correctness/lifecycle/fallback vẫn đạt acceptance và không mở rộng Phase B/C chỉ để làm đẹp benchmark.

### Validation cuối

Đã chạy tuần tự trên working tree hiện tại:

- `cargo fmt --check` — PASS
- `cargo check --workspace` — PASS
- `cargo test --workspace` — PASS
- `cargo clippy -p chatcmd-runtime -p chatcmd-storage -p chatcmd-mcp --all-targets -- -D warnings` — PASS

Sau các fix cuối về directory rename/path normalization, target Plan 20 được chạy lại và PASS 16/16. Full workspace validation cuối cũng PASS toàn bộ với exit status 0.

### Phases

- Phase A path/metadata index + batch tools: **complete**.
- Phase B text/trigram index: deferred.
- Phase C symbol index: deferred.
