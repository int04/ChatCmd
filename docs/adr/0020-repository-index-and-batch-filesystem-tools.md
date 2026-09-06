# ADR 0020: Repository metadata index and batch filesystem tools

## Status

Accepted for Phase A foundations. Indexed `find` and search candidate selection remain gated on reconciliation benchmarks.

## Decision

ChatCMD maintains one generation-numbered path/metadata index per canonical workspace root. The index stores paths, entry type, size, modified time, identity/version metadata, extension, ignore state, and the generation in which an entry was last observed. It never stores file content by default.

SQLite schema 17 reserves durable index state and entries outside the task timeline. Paths have both a byte representation and a lossy display value. The runtime implementation currently maintains the active snapshot in memory and marks it stale after native mutations. A complete crawl publishes a new generation atomically; a failed or cancelled crawl keeps the previous generation and reports an error/unknown freshness.

The index is only an accelerator. Exact stat and read results always use the authorized direct filesystem paths and current version logic. Until watcher overflow, restart reconciliation, and durable hydration are complete, `find` and search continue to use their bounded direct walkers. This preserves correctness when the index is absent, stale, corrupt, or migrating.

`fs_batch_stat` and `fs_batch_read` preserve input order and return a structured outcome per item. They enforce hard item caps. Batch reads invoke the existing streaming v2 reader with bounded concurrency and enforce an aggregate output cap. Duplicate paths remain duplicate result items. Each item independently passes canonical-scope authorization.

## Consistency and recovery

- Generation increases only after a complete crawl succeeds.
- Native mutations mark matching root indexes stale.
- Cancelled, over-limit, or failed builds do not replace the last complete generation.
- SQLite foreign-key cleanup removes entries when a workspace index is removed.
- Basic filesystem operations fall back directly and never require the index.
- Content and symbol indexing are deferred; excluded or secret content is not persisted.

## Limits and follow-up gates

Initial crawl is capped at 1,000,000 entries and is cancellable. Before enabling indexed `find`, the runtime still needs durable hydration, batched SQLite publication, watcher event application/overflow handling, periodic reconciliation, quota enforcement based on database size, and the required 100k/1m benchmark suite.
