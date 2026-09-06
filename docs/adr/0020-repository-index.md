# ADR 0020 — Bounded repository metadata index

## Status

Accepted for Plan 20 Phase A. Phase B text indexing and Phase C symbol indexing remain deferred until Phase A benchmark evidence justifies them.

## Decision

ChatCMD keeps a path/metadata-only repository index as an accelerator. Direct filesystem tools remain the correctness source of truth. Each workspace root has a monotonically increasing generation, explicit freshness (`fresh`, `stale`, `unknown`), entry count, indexed byte count and schema version.

The runtime builds snapshots with the shared ignore-aware walker and hard limits (one million entries plus cancellation/time controls inherited from the tool context). A successful explicit rebuild is persisted to the existing SQLite database in one transaction using migration 0017. Persisted snapshots store relative path bytes plus display paths and metadata, never source file contents.

On startup, persisted snapshots are loaded only for currently authorized roots and are restored as stale candidates. Inactive roots are removed from SQLite. A bounded background rebuild/reconcile verifies the live filesystem and republishes a fresh snapshot. Corrupt, incompatible or failed indexes must not make basic filesystem tools unavailable; direct scan/stat/read remains the fallback.

Native filesystem mutations mark the in-memory index and the persisted root stale after the mutation commits. This prevents a previously fresh snapshot from being presented as authoritative between mutation and reconcile. External/watcher changes are still treated as best-effort signals; restart reconcile and live filesystem verification remain required because watcher delivery is not a correctness boundary.

## Storage and quota

SQLite is reused to avoid another embedded database dependency and packaging/locking model. `workspace_repository_indexes` stores root state/generation and `workspace_repository_index_entries` stores path metadata. Rebuild publication uses a single transaction: upsert root, replace entries, commit. WAL/busy-timeout behavior comes from the existing repository configuration.

The runtime caps a snapshot at 1,000,000 entries. Persistence additionally rejects an approximate metadata payload above 512 MiB and measures SQLite page growth inside the replacement transaction, rolling the transaction back if repository-index growth exceeds the 2 GiB hard disk quota. These are hard safety limits rather than tuning targets. Full source content is not stored by Phase A.

## Batch tools

`fs_batch_stat` preserves input order and per-item errors with a hard 500-item cap. Exact stat/version results always come from the live filesystem; the metadata index only contributes diagnostics/candidate verification. Aggregate hash-byte budget, metadata-call budget, cancellation and one common wall-clock deadline are enforced across the batch. `fs_batch_read` preserves input order, uses the streaming `fs_read_text_v2` path, caps requests at 50, bounds concurrency, allocates from one aggregate read-byte budget, uses a common wall-clock deadline/cancellation-aware admission, and enforces a hard aggregate output cap.

## Consistency and fallback

1. A successful crawl creates generation N and marks it fresh.
2. A committed native mutation marks matching roots stale in memory and SQLite.
3. Persisted snapshots are restored as stale after restart regardless of their stored state.
4. Startup reconcile performs a fresh bounded crawl and persists generation N+1.
5. If the index is missing, stale, incompatible or fails to rebuild, direct filesystem operations continue to work.

## Indexed query path

When the root index is fresh and request options are compatible with index semantics, `fs_find_v2` enumerates indexed path candidates and `fs_search_v2` enumerates indexed file candidates. Both paths live-verify size, modified time and entry type before returning/reading a candidate. A mismatch marks the root stale; an initial query retries with the bounded direct walker, while a continuation fails with typed `cursor_stale` rather than silently mixing generations. Requests with custom excludes or `includeIgnored=true` use the direct walker to preserve ignore/filter parity. Search content matching always uses the existing streaming reader; Phase A never treats indexed metadata as a content match.

Watcher events update/tombstone affected in-memory metadata only as an immediate hint and always mark the generation stale. Missing/renamed paths use bounded exact tombstones rather than scanning an arbitrarily large descendant subtree; stale-state direct fallback plus periodic reconcile removes remaining descendants. Watcher errors/creation failures also mark the root stale. Periodic reconcile is started once per runtime host, and per-root rebuild gates prevent overlapping rebuilds. The current ignore model has no dynamic runtime ignore configuration; persisted `ignore_fingerprint='default-v1'` therefore denotes the shared default ignore policy. If dynamic ignore configuration is introduced later, its fingerprint must invalidate/rebuild the snapshot.

SQLite replacement uses page-count/page-size growth accounting before commit and rolls back a replacement that exceeds the hard quota, preserving the previous snapshot. A best-effort `PRAGMA wal_checkpoint(PASSIVE)` runs after successful replacement to keep WAL growth bounded without forcing `VACUUM` or a blocking truncate on every rebuild. The quota is a logical SQLite growth bound; transient WAL/filesystem overhead can still differ from that estimate.

The reconcile loop currently has no repository-index-specific shutdown token. Its watcher handles are retained by the runtime host and its Tokio tasks terminate with runtime teardown; an explicit graceful-shutdown token can be added if the host later requires independent indexer shutdown semantics.

## Benchmark evidence

The 100k synthetic workload previously measured fixture construction at about 9,306 ms, cold rebuild at about 679 ms, indexed warm find p50/p95 at about 163/172 ms versus direct late-match p50/p95 at about 311/313 ms, incremental updates at about 0.5 ms for 1 change, 10.7 ms for 100 changes and 1,155 ms for 10,000 changes, with peak RSS about 59.8 MB. SQLite persistence at 100k measured about 6,201 ms write, 907/951 ms load p50/p95, about 60.3 MB DB and 61.4 MB WAL.

The required 1,000,000-path benchmark also passed the harness: fixture build about 150,505 ms; cold rebuild about 67,320 ms; indexed warm find p50/p95 about 1,662/1,669 ms; direct late-match p50/p95 about 297/304 ms; incremental 1/100/10,000 changes about 2.565/39.113/1,977.936 ms; batch 500 about 236.190 ms versus sequential 500 about 57.854 ms; peak RSS about 752,975,872 bytes; total benchmark runtime about 700.23 s.

The 1m result is intentionally documented as a negative performance finding: Phase A indexed candidate enumeration currently materializes/filter/sorts in-memory metadata and is slower than the direct synthetic late-match workload at that scale. Correctness, lifecycle and fallback acceptance still hold, but no claim is made that the indexed path always outperforms direct traversal. Phase B/C are not expanded merely to make this benchmark look better.

## Deferred work

Phase B text/trigram indexing and Phase C symbol indexing remain deferred. Phase A deliberately stores no source content and keeps direct filesystem traversal/streaming as the correctness fallback.
