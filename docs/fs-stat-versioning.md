# Filesystem version tokens

`fs_stat` accepts the legacy `{ "path": ... }` request and defaults to the fast
`metadata` strength. Its result retains the legacy `name`, `path`, `entryType`,
`size`, and `readonly` fields, and adds `sizeBytes`, nanosecond timestamps,
permissions, and a signed `versionToken`.

The token is an opaque, HMAC-SHA-256 authenticated `v1` value. It binds a
canonical scope plus root-relative path fingerprint, an OS file-identity fingerprint, entry
kind, size, high-resolution modified time (plus Unix change time), requested strength, and optional
content digest. The process-local signing key is generated when the workspace
service starts and shared by its scoped clones. Restarting the service rotates
the key, so older tokens return `versionUnsupported`. Tokens contain neither the
key nor raw canonical paths, Unix device/inode values, or Windows volume/file IDs.

Strengths:

- `metadata` performs no content reads. It detects replacement through Unix
  device/inode identity or Windows volume/file ID, and ordinary modifications
  through size and the highest-resolution modified timestamp exposed by the OS.
- `sampled` additionally hashes bounded blocks from the beginning, middle, and
  end. It is probabilistic and is not a substitute for `content` when every byte
  matters.
- `content` streams the complete file through SHA-256 using a fixed-size buffer.

Hash modes honor `budget.timeoutMs`, `budget.maxBytesRead`, and request
cancellation. Metadata is captured both before and after hashing;
`fileChangedDuringHash` is returned when identity, kind, size, or timestamp
changes. Filesystems with coarse timestamp granularity can miss an in-place,
same-size edit in metadata mode; use content mode where that residual risk is
unacceptable. Sparse content mode remains proportional to logical file size.

Symbolic links, broken links, Windows junctions/reparse points, and traversal
through them remain rejected by workspace path policy. Directories and special
entries can be captured in metadata mode, but content hashing requires a readable
regular file.

`WorkspaceService::verify_expected_version` is the shared integration point for
the write/edit/delete/move preconditions in Plans 09, 11, and 12. It distinguishes
`targetMissing`, `targetReplaced`, `versionMismatch`, and
`versionUnsupported` without requiring callers to parse token payloads.
