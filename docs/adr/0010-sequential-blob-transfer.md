# ADR 0010: Sequential owner-scoped blob transfer

## Status

Accepted for the initial large-content implementation.

## Context

MCP JSON requests are bounded and Base64 adds roughly one third of transfer overhead. Large
`fs_write_text`, `fs_write_raw`, and `fs_apply_edits` arguments therefore cannot reliably travel
as one request. Persisting those arguments also duplicates sensitive and potentially very large
content in the task timeline.

## Decision

ChatCMD uses sequential uploads for the first protocol version:

`blob_begin` → `blob_write_chunk` → `blob_status`/resume → `blob_seal` → consumer or `blob_abort`.

Each upload receives an opaque `blob:v1:<uuid>` content reference. The reference contains no path.
Metadata is held server-side and binds the upload to agent, task, turn, and purpose. Every operation
rechecks ownership, expiry, state, size, and integrity. Chunks are limited to 1 MiB, offsets must
equal `nextOffset`, and an identical chunk retry is idempotent. A different retry at an already
written offset is a conflict. Sealing streams the file through SHA-256 and makes it immutable.

Temporary bytes live beside managed application data rather than under the workspace. Startup
reconciles orphan files, while TTL and explicit abort provide cleanup. Quotas bound individual
blobs, owner reservations, concurrent owner uploads, and global reservations. Consumers take an
exclusive lease, stream into the filesystem mutation's same-directory temporary writer, and either
mark the blob consumed or return it to sealed state after a failed commit.

Inline text remains available up to 256 KiB. Inline Base64 is bounded to the corresponding encoded
size. Tool inputs enforce exactly one inline value or `contentRef` at runtime. Timeline persistence
redacts inline content, edit arrays, chunk bytes, and integrity values; references and summaries
remain observable.

For `fs_apply_edits`, the referenced blob contains a JSON array of `TextEdit` values. This removes
the MCP envelope bottleneck, although the current edit engine still materializes that array when it
resolves and validates ranges.

## State machine

`uploading → sealed → consuming → consumed`

Failures before target commit return `consuming → sealed`. Abort or expiry transitions active data
to `aborted` and removes temporary bytes. Sealed and consumed uploads reject further chunks.

## Consequences

Sequential transfer has a simple bounded resume contract and incremental disk writes, but does not
support parallel/out-of-order chunks. Metadata is process-local in this MVP; after a restart,
orphan bytes are removed rather than resumed. A future version can persist metadata transactionally
if restart-resume becomes a requirement.
