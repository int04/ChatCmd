# Scoped approval grants

ChatCMD keeps approval decisions bounded to one agent and task. A child task never queries a
parent grant; delegation therefore cannot widen authority. `inherited_from` is reserved for an
explicit future intersection flow and is constrained by a foreign key.

## Risk matrix

| Class | Examples | Reusable safe-read grant |
| --- | --- | --- |
| Metadata read | `workspace_roots`, `fs_stat`, `fs_list`, `fs_batch_stat` | Yes |
| Content read | `fs_read_text`, `fs_batch_read` | Yes |
| Compute read | `fs_find`, `fs_search` | Yes |
| Create/modify | writes, edits, index rebuild | No |
| Move/copy | `fs_move`, `fs_copy` | No |
| Destructive | `fs_delete`, `process_kill` | No |
| Process execution | shell and Git operations | No |
| Privileged | task and agent control tools | Never through execution approval |

The MCP catalog is the single source of truth for the risk class, path roles, budget support,
dry-run support, expected-version support, and whether execution approval is required.

## Grant matching and budget precedence

A reusable grant binds the exact agent, task, child attempt (when applicable), catalog hash, explicit tool set, canonical exact-file
or directory-subtree scopes, safe options, expiry, call count, files scanned, and bytes read.
Request budgets are reserved atomically before dispatch and take precedence over defaults. A call
outside the scope, with ignored/hidden traversal enabled, or beyond any remaining budget prompts
again. Catalog changes invalidate matching. Restart preserves only unexpired persisted grants;
task stop, agent disable, credential rotation, deletion, expiry, exhaustion, and user revocation
invalidate them.

## Approval summaries

Pending mutation approvals persist and display paths, operation, path count, overwrite/recursive
mode, expected version, dry-run/budget values, and a byte estimate. Inline content is never stored
in the approval or audit log. The approval stores a SHA-256 digest of the complete operation, so a
changed operation is a different request. Audit rows contain identifiers, counters, path counts,
and bounded reasons only—not paths, command output, file content, or secrets.
