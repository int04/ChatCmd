# ADR 0002: Unified tool result envelope and opaque cursors

- Status: Accepted
- Date: 2026-09-03

## Context

ChatCMD tools historically return tool-specific top-level JSON shapes. This is simple for small outputs but makes large-project behavior ambiguous: callers cannot consistently distinguish complete output from paged output, intentional truncation, content externalization, or non-fatal warnings. Cursor semantics are also tool-specific, which makes accidental cursor reuse across scopes possible unless each tool independently implements the same safety rules.

Changing every existing result shape in place would break connectors, UI parsing, and cached schemas. The project therefore needs a shared contract that can be adopted incrementally.

## Decision

1. Define `ToolResultEnvelope<T>` in `chatcmd-runtime` as the canonical successful-result contract. Version 1 contains `schemaVersion`, typed `data`, optional `page`, optional `truncation`, optional `usage`, optional warnings, and optional `contentRef`.
2. Keep failures on the existing typed `RuntimeError` path. Partial successful output is expressed through envelope metadata, not an error-shaped payload.
3. Define stable camelCase truncation reasons and test their serialized representation.
4. Introduce a common opaque cursor codec using URL-safe Base64 plus HMAC-SHA256. Cursors bind a cursor version, tool kind, hash of canonical normalized scope, typed continuation state, and optional expiry. Raw scope and signing material are never embedded.
5. Introduce the contract additively. Existing `fs_list` retains its array result and offset/limit input. New `fs_list_v2` demonstrates the envelope and opaque cursor contract without breaking old consumers.
6. Advertise result compatibility through the generated MCP catalog. Envelope-enabled tools expose a generated `resultSchema` and `capabilities.resultSchemaVersion`; tools that have not migrated leave those fields unset/null.
7. Update UI formatting incrementally so migrated tools expose meaningful continuation/truncation/content-reference state while legacy parsers continue to work.

## Consequences

### Positive

- Callers can reliably detect pagination and truncation without inspecting tool-specific payloads.
- Result schemas are typed and generated from Rust types, reducing schema drift.
- Cursor scope/tool mismatches fail closed with stable error codes.
- Migration does not silently change existing result shapes.
- Future filesystem, Git, shell, process, task/artifact, and skill migrations can reuse one contract.

### Trade-offs

- During migration, clients must support both legacy and envelope-enabled tools.
- `fs_list_v2` uses offset continuation internally and therefore does not provide snapshot-consistent directory traversal when a directory changes between pages. Large-repository paging/indexing is a separate optimization plan.
- Cursor signing uses an ephemeral runtime-process key, so cursors are intentionally invalid after process restart.
- `contentRef` is part of the common contract even though the first migrated sample does not currently need to externalize directory-list content.

## Compatibility rule

Never replace a legacy top-level result shape in place unless every consumer has an explicit compatibility path. Prefer a versioned tool name or another catalog-visible migration mechanism, and remove the legacy form only in a separately planned breaking change.

## Validation

The contract must retain tests for stable serialization, generated JSON schema, omitted optionals, truncation reason names, output-byte accounting, cursor tamper/scope/version/expiry behavior, and legacy `fs_list` compatibility. Catalog tests must verify that `fs_list` and `fs_list_v2` advertise distinct input/result contracts.
