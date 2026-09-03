# ADR 0012: Staged filesystem mutations and recovery journals

## Status

Accepted for the local runtime safety baseline. SQLite-backed startup recovery and destructive-scale benchmarks remain follow-up validation items recorded in the checked Plan 12 file.

## Decision

`fs_copy` and `fs_move` preflight the complete source tree within caller budgets, reject overlapping trees and reparse points, then copy into a uniquely named sibling staging path. Verification is `none`, `metadata`, or SHA-256 `content`. The destination becomes visible only through a sibling rename. Replacement first renames the old destination to an operation-owned backup; it is restored when publish fails and removed only after publish succeeds.

A move removes its source only after staging verification and destination publish. Failure to remove the source is reported as `completedWithSourceRemaining`; it is never reported as an unqualified completed move.

`fs_delete` defaults to same-filesystem quarantine. Permanent deletion is explicit and walks entries with no-follow metadata checks. Workspace roots and explicit grant roots remain undeletable and unmovable.

Every mutating execution receives a UUID operation ID. Runtime sidecar journals are atomically replaced and synced at phase boundaries. Migration 0015 supplies the normalized SQLite operation journal for host-level recovery. Journal records contain paths, owner identity, state, counters, rollback actions, timestamps, and errors, never file content.

## Guarantees by filesystem

- Same local filesystem: staging publish is atomic to observers when the platform's rename implementation provides the normal same-volume guarantee. Existing destinations remain available until the replacement is ready.
- Cross-device source: copy/move staging is created beside the destination, so publish remains a same-filesystem rename. Move source cleanup follows publish.
- Network/removable filesystems: preflight, verification, ordering, and reporting still apply, but atomic rename, flush durability, stable identity, and crash persistence are only as strong as the mounted filesystem.
- Metadata preservation currently covers permissions. Creation/modification timestamps, ACLs, sparse extents, hard-link topology, and alternate streams are best-effort or unsupported.

## State model

Transfers use `planned -> staging -> verifying -> readyToPublish -> published -> removingSource -> completed`. Terminal variants are `failedRolledBack`, `failedPartial`, `cancelledNoChange`, `cancelledRolledBack`, `cancelledPartial`, and `completedWithSourceRemaining`.

Delete uses `planned -> quarantining|deleting -> completed`. A quarantine warning identifies the retained recovery path.

## Compatibility

The legacy `overwrite` boolean maps to `conflictPolicy=replace`; false/omitted maps to `error`. The typed contract adds dry-run, expected source/destination versions, verification, metadata policy, no-follow policy, atomic publish, and bounded timeout/file/byte budgets. Directory merge is intentionally not offered because safe merge rollback semantics are not yet defined.

