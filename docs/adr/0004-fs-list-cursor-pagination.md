# ADR 0004: Scalable cursor pagination for directory listing

- Status: Accepted
- Date: 2026-09-03

## Context

The compatibility `fs_list` path enumerates a directory, fetches metadata for every entry, materializes all entries, globally sorts by name, and only then applies offset/limit. Plan 02 introduced `fs_list_v2` and an opaque signed cursor, but its continuation state was only an offset and it still called the legacy implementation. A request for one page therefore still had O(N) traversal/metadata/sort cost and repeated that work for every page.

Large directories need bounded work per page without silently promising a global alphabetical order that requires materializing or externally indexing the full directory.

## Decision

1. `fs_list` remains the compatibility API. It keeps its legacy array result and offset/limit semantics, including global name sorting, but runtime dispatch caps `limit` to 2,000. New large-directory callers should use `fs_list_v2`.
2. `fs_list_v2` uses Strategy B: a server-side continuation state retains an open `std::fs::ReadDir` iterator. The opaque HMAC-signed cursor contains only a random `stateId` plus `directoryVersion`; the existing cursor codec also binds the token to tool kind and canonical path scope.
3. V2 ordering is explicitly `sort: "filesystem"`. It is traversal order from the operating system and is not a global alphabetical contract.
4. The continuation state additionally binds canonical path, sort, requested metadata fields, and `includeHidden`. Reusing a cursor with different options fails with `cursor_scope_mismatch`.
5. Each page stores at most the page items plus one pending `DirEntry` used to determine `hasMore`. It does not collect or sort the whole directory.
6. Metadata is opt-in. `metadata: []` performs no per-entry metadata stat calls. `type` uses `DirEntry::file_type`; `size` and `readonly` use `symlink_metadata` and count against `maxStats`.
7. Traversal checks cancellation, elapsed-time budget, entry-scan budget, and stat budget inside the blocking loop. Budget stops return a successful partial envelope with `truncation` and a continuation cursor.
8. Per-entry enumeration/type/stat failures are non-fatal warnings when the directory can otherwise continue. Warning count is bounded.
9. The result includes `directoryVersion`. Continuation revalidates the current directory metadata version; a detected mutation fails with `directory_changed` and requires restarting from page 1.
10. Continuation state expires after 15 minutes and the runtime keeps at most 128 active directory cursors. Expired/evicted state fails with `cursor_expired`. Runtime restart also invalidates cursors because both in-memory state and the ephemeral cursor signing key are lost.
11. Non-UTF-8 names/paths are returned through lossy UTF-8 conversion and the item sets `nameEncodingLossy: true` so callers can see that representation was lossy.

## Input contract

```json
{
  "path": "D:/repo/src",
  "cursor": "opaque-cursor",
  "limit": 200,
  "sort": "filesystem",
  "metadata": ["type", "size", "readonly"],
  "includeHidden": true,
  "budget": {
    "timeoutMs": 5000,
    "maxEntriesScanned": 10000,
    "maxStats": 1000
  }
}
```

Defaults are `limit=200`, `sort=filesystem`, `metadata=[]`, `includeHidden=true`, `timeoutMs=5000`, `maxEntriesScanned=10000`, and `maxStats=1000`. Runtime clamps page size to 1..2,000.

## Result contract

```json
{
  "schemaVersion": 1,
  "data": {
    "items": [
      {
        "name": "lib.rs",
        "path": "D:/repo/src/lib.rs",
        "entryType": "file",
        "size": 1234,
        "readonly": false
      }
    ],
    "directoryVersion": "sha256:...",
    "sort": "filesystem"
  },
  "page": {
    "nextCursor": "opaque-cursor",
    "hasMore": true
  },
  "usage": {
    "elapsedMs": 1,
    "entriesScanned": 201,
    "metadataCalls": 0,
    "outputBytes": 4096
  }
}
```

Optional item metadata fields are omitted unless requested. Budget/cancellation stops add `truncation`; skipped/unavailable entries can add `warnings`.

## Consequences

### Positive

- First-page and next-page work is bounded by page/budget rather than directory cardinality.
- Stable directories can be consumed page-by-page without duplicates or omissions from re-sorting/re-scanning.
- Metadata cost is explicit and measurable.
- Cursor misuse across path/options fails closed.
- Legacy callers remain compatible.

### Trade-offs

- V2 does not provide global alphabetical ordering.
- Continuations are process-local and intentionally short-lived.
- `directoryVersion` is a best-effort filesystem metadata version, not a transactional filesystem snapshot. When a mutation is detected, callers restart rather than trying to merge generations.
- A platform may expose filesystem changes with timestamp granularity/metadata behavior that is not a perfect mutation journal; callers must not treat the version as a durable file-system transaction ID.

## Validation

Plan 04 adds tests for empty/single/boundary pages, 10k+ entries, stable multi-page coverage, cursor option binding, detected directory mutation, bounded first-page scan, metadata-free listing, cancellation/time budgets, and legacy sorted offset behavior.

Windows benchmark on 2026-09-03 with 100,000 entries and `limit=200`:

- first page: 16,320 µs, 201 entries scanned;
- next page: 4,735 µs, 201 entries scanned.

The fixture deliberately reports scan counts because they demonstrate the cardinality-independent page invariant more reliably than machine-specific wall-clock time.
