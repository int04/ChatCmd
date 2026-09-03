# ADR 0014: Bounded turn file-change tracking

## Status

Accepted.

## Context

The original tracker created a recursive operating-system watcher for every turn and read whole files before truncating their UI snapshot. Restore and build commands in a monorepo could therefore cause an unbounded event storm, while native filesystem tools still routed full private `__chatcmdDiff` payloads through their public result.

## Decision

- Starting a turn only allocates in-memory state. A recursive watcher starts lazily immediately before `shell_create` and stops at turn completion.
- Native write, raw write, replace, range edit, copy, move, delete, and directory-create operations publish a typed `FileChangeRecord` after their commit point. Their public tool output contains no private diff payload.
- Watcher callbacks only use non-blocking enqueue into a 4,096-event bounded channel. A worker debounces for 100 ms, coalesces at most 8,192 events per batch, snapshots outside the tracker mutex, and emits deterministic path-sorted records.
- Queue/backend loss increments a counter. Completion sets `fileChangeTrackingIncomplete`, reports `fileChangeEventsDropped`, and marks watcher records `unknownDueToOverflow` rather than claiming exact results.
- Snapshots stat first. Files up to 200,000 bytes are read completely. Larger files read a bounded prefix and suffix totaling 200,000 bytes. Invalid UTF-8 is metadata-only. Sampled snapshots do not report misleading line counts.
- The shared `WorkspaceIgnorePolicy` excludes generated trees. Paths created and removed within one debounce window are coalesced away, including atomic staging files. Native records win over watcher duplicates.
- The completion payload uses schema version 2. The UI consumes typed preview/confidence data and shows unknown line counts as `?`.

## Consequences

Idle/native-only turns no longer consume an OS watcher. Native mutations have exact commit ordering and version/size metadata. Shell tracking is bounded and explicitly reports degradation. Directory-scale copy/move/delete records use the mutation operation's artifact reference when supplied; they do not materialize one in-memory record per descendant.

Platform watcher backends can still coalesce or omit events before the application callback. Such loss is only detectable when the backend reports an error or the bounded queue drops data; a future persistent workspace index can provide full budgeted reconciliation after silent backend loss.
