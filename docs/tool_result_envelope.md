# Unified Tool Result Envelope

Status: implemented as the shared result contract in `chatcmd-runtime`, with `fs_list_v2` as the first compatibility-safe consumer.

## Why this exists

Large-project tools need a predictable way to report successful data, pagination, intentional truncation, lightweight usage metrics, warnings, and externally stored content. Before this contract, each tool returned its own top-level shape, so callers had to infer whether output was complete and how to continue.

The envelope is intentionally additive. Existing tool shapes are not silently changed. A tool only uses this contract when its catalog metadata advertises `resultSchemaVersion`.

## Contract

The shared Rust type serializes with camelCase field names:

```json
{
  "schemaVersion": 1,
  "data": {},
  "page": { "nextCursor": "opaque-token", "hasMore": true },
  "truncation": {
    "truncated": true,
    "reason": "outputLimit",
    "returnedItems": 100,
    "omittedItems": 25
  },
  "usage": {
    "elapsedMs": 12,
    "filesScanned": 20,
    "bytesRead": 4096,
    "bytesWritten": 0,
    "outputBytes": 512
  },
  "warnings": [{ "code": "example_warning", "message": "Example warning." }],
  "contentRef": {
    "id": "artifact-or-content-id",
    "mediaType": "application/json",
    "sizeBytes": 123456
  }
}
```

Only `schemaVersion` and `data` are always serialized. Optional metadata is omitted when it has no meaning for the result; `warnings` is omitted when empty.

### Field semantics

- `schemaVersion`: version of the result envelope contract. Version 1 is current.
- `data`: tool-specific typed payload.
- `page`: continuation state for intentionally paged results. `hasMore=true` is accompanied by a server-generated `nextCursor`.
- `truncation`: successful but incomplete result. It is distinct from pagination and records why requested material was not returned inline.
- `usage`: best-effort execution/output accounting. `outputBytes` is the exact byte length of the serialized JSON envelope after the field has stabilized.
- `warnings`: non-fatal conditions.
- `contentRef`: opaque reference to complete content kept outside the inline result. Consumers must not derive a filesystem path from it.

## Stable truncation reasons

Version 1 defines these camelCase values:

- `outputLimit`
- `itemLimit`
- `timeBudget`
- `fileBudget`
- `metadataBudget`
- `byteBudget`
- `cancelled`
- `replayEvicted`
- `binaryContent`
- `contentExternalized`

A new reason is a contract change and must be reflected by tests/catalog versioning.

## Pagination and opaque cursors

`fs_list_v2` is the first tool using the shared cursor codec. Callers treat cursors as opaque strings.

A cursor carries only signed continuation state needed by the server: cursor version, tool kind, SHA-256 hash of the normalized canonical scope, tool-specific state, and optional expiry. The payload is URL-safe Base64 and protected with HMAC-SHA256. The HMAC key is process-local and is never serialized. Raw canonical scope/path is not embedded; only its hash is present. A cursor therefore cannot be reused for another tool or another path even when continuation state looks valid.

Stable cursor errors:

- `invalid_cursor`: malformed state, invalid signature, or invalid typed continuation state;
- `cursor_scope_mismatch`: cursor belongs to another tool or canonical scope;
- `cursor_expired`: cursor passed its declared expiry;
- `cursor_version_unsupported`: cursor was produced by an unsupported cursor version.

`fs_list_v2` now uses a process-local server continuation state (ADR 0004) containing an open `ReadDir` iterator. Its signed opaque tool state contains a random state id plus `directoryVersion`; server state binds path, sort, metadata fields, and hidden-file filter. State expires after 15 minutes and is also bounded by an active-state cap. Runtime restart invalidates both state and the ephemeral signing key. A detected directory-version change fails continuation with `directory_changed` so the caller restarts from page 1.

## Success, partial success, errors, and cancellation

Use the envelope only for successful tool results. Runtime failures continue through the existing typed `RuntimeError` path and must not be encoded as successful `data`.

- Complete success: return `data` with no `page`/`truncation` unless useful metadata is present.
- Paged success: set `page.hasMore` and return server-generated `page.nextCursor` when more data exists.
- Budget-limited success: return usable `data` plus `truncation.truncated=true` and a stable reason.
- Externalized success: return bounded inline data plus `truncation.reason=contentExternalized` and `contentRef`.
- Cancellation before a useful coherent result exists remains an error. A tool that can deliberately preserve a coherent partial result may use `truncation.reason=cancelled`.

Consumers must not infer truncation from array length, output byte length, missing fields, or error strings.

## Examples

### Complete result

```json
{
  "schemaVersion": 1,
  "data": [
    { "path": "D:/repo/src", "name": "src", "entryType": "directory", "size": 0, "readonly": false }
  ],
  "usage": { "outputBytes": 167 }
}
```

### Paged `fs_list_v2` result

```json
{
  "schemaVersion": 1,
  "data": {
    "items": [
      { "path": "D:/repo/a.rs", "name": "a.rs", "entryType": "file", "size": 128 }
    ],
    "directoryVersion": "sha256:...",
    "sort": "filesystem"
  },
  "page": { "nextCursor": "eyJ2ZXJzaW9uIjoxLC4uLg.signature", "hasMore": true },
  "usage": { "elapsedMs": 2, "entriesScanned": 101, "metadataCalls": 100, "outputBytes": 410 }
}
```

Continue only by sending that opaque cursor back to the same tool and path/options:

```json
{
  "path": "D:/repo",
  "cursor": "eyJ2ZXJzaW9uIjoxLC4uLg.signature",
  "limit": 100,
  "sort": "filesystem",
  "metadata": ["type", "size"]
}
```

### Budget-truncated result

```json
{
  "schemaVersion": 1,
  "data": { "matches": ["bounded inline result"] },
  "truncation": {
    "truncated": true,
    "reason": "byteBudget",
    "returnedItems": 1,
    "omittedItems": 42
  },
  "usage": { "bytesRead": 1048576, "outputBytes": 241 }
}
```

### Artifact/content-backed result

```json
{
  "schemaVersion": 1,
  "data": { "preview": "First bounded portion..." },
  "truncation": {
    "truncated": true,
    "reason": "contentExternalized",
    "returnedItems": 1
  },
  "contentRef": {
    "id": "artifact-01JXYZ",
    "mediaType": "text/plain",
    "sizeBytes": 5242880
  },
  "usage": { "outputBytes": 319 }
}
```

## Migration inventory

| Tool family | Current top-level result shape | Envelope status |
|---|---|---|
| `fs_list` | plain array of `FsEntry` with caller-provided `offset`/`limit` | Legacy shape preserved; runtime page-size cap is 2,000. |
| `fs_list_v2` | `ToolResultEnvelope<FsListPageData>` with streaming opaque cursor | Migrated and optimized by Plan 04; `resultSchemaVersion=1`. |
| filesystem search/find/read/stat/mutations | tool-specific arrays/objects | Not migrated in this plan. |
| Git tools | command/output-specific objects or arrays | Not migrated in this plan. |
| shell replay/session tools | typed shell objects with shell-specific replay state | Not migrated in this plan. |
| process tools | process arrays/objects | Not migrated in this plan. |
| task/artifact tools | task-domain JSON values | Not migrated in this plan. |
| skills | skill arrays/read objects | Not migrated in this plan. |

Remaining migrations should happen tool-by-tool, with explicit compatibility and performance work. Consumers must continue supporting the old shape for tools whose catalog entry has no `resultSchemaVersion`.

## Catalog and client behavior

Catalog version 2 includes `resultSchema` for envelope-enabled tools and nullable `capabilities.resultSchemaVersion`. `fs_list_v2` advertises version 1; legacy `fs_list` keeps its unchanged input schema and has no result schema version.

Recommended client behavior:

1. Revalidate the generated catalog using the existing `catalogHash` flow.
2. If `resultSchemaVersion` exists, validate/render the envelope by that version.
3. If absent, continue using the tool's legacy result parser.
4. Never parse, construct, mutate, or synthesize continuation cursors.
5. Surface `page.hasMore`, truncation reason, and `contentRef` availability instead of hiding them in raw JSON.

## Implementation locations

- Contract/cursor codec: `crates/chatcmd-runtime/src/tool_result.rs`
- First runtime integration: `src/runtime_host/dispatch.rs` (`fs_list_v2`)
- MCP registration: `crates/chatcmd-mcp/src/lib.rs`
- Catalog result schema/capabilities: `crates/chatcmd-mcp/src/tool_catalog.rs`
- UI formatting: `web/src/tasks/taskToolOutput.ts`
- Decision record: `docs/adr/0002-unified-tool-result-envelope.md`
