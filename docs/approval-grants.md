# Scoped approval grants

ChatCMD keeps approval decisions bounded to one agent and task. Child tasks receive no approval
authority implicitly. A parent may explicitly request a child safe-read grant, but the runtime
creates it only as a bounded intersection of one active parent grant: requested tools must remain
approval-required safe reads, canonical path scopes must stay inside the parent scopes, and child
budgets are atomically reserved from the parent's remaining budget. The child grant records
`inherited_from`, binds the current child attempt, and expires no later than either the parent grant
or the child lease. Terminal child transitions revoke any remaining active child grants.

## Risk matrix

| Class | Examples | Reusable safe-read grant |
| --- | --- | --- |
| Metadata read | `workspace_roots`, `fs_stat`, `fs_list`, `fs_batch_stat` | Yes |
| Content read | `fs_read_text`, `fs_batch_read` | Yes |
| Compute read | `fs_find`, `fs_search` | Yes |
| Create/modify | writes, edits, index rebuild | No |
| Move/copy | `fs_move`, `fs_copy` | No |
| Destructive | `fs_delete`, `process_kill` | No |
| Process execution | `command_run`, shell and Git operations | No |
| Privileged | task and agent control tools | Never through execution approval |

The MCP catalog is the single source of truth for the risk class, operation class, path roles,
budget support, dry-run support, expected-version support, and approval UI hint. Runtime
authorization uses the operation class independently from the UI hint. Unknown tools fail closed
as permission changes until they are explicitly classified.

`command_run` and `shell_create` are process execution. `shell_create` remains execution even when no arguments are supplied: shell startup and profile
loading can execute code. In deny mode it cannot create a process. In approval mode the exact
operation must be approved before spawn and is rechecked immediately before dispatch. Lifecycle
reads and owner-scoped stop/cleanup controls remain available so a denied task can still report its
state and release resources.

MCP callers cannot change task execution mode. `task_set_execution_mode` remains available as a
compatibility adapter but returns `permission_change_requires_user`; only the encrypted,
management-header-protected local UI API may persist a mode change. A UI change writes a
server-generated audit event, cancels pending approvals, and revokes active grants for the task and
its descendants. An approved shell runs with the operating-system rights of the ChatCMD process.
Working-directory validation and approval are not an OS filesystem or network sandbox.

## Grant matching and budget precedence

A reusable grant binds the exact agent, task, child attempt (when applicable), catalog hash, explicit tool set, canonical exact-file
or directory-subtree scopes, safe options, expiry, call count, files scanned, and bytes read.
Request budgets are reserved atomically before dispatch and take precedence over defaults. A call
outside the scope, with ignored/hidden traversal enabled, or beyond any remaining budget prompts
again. Catalog changes invalidate matching. Restart preserves only unexpired persisted grants;
task stop, agent disable, credential rotation, deletion, expiry, exhaustion, and user revocation
invalidate them.

Execution consent from `agent_plan_question` is separate from approval grants. Old clarification
answers, custom text, timeout/reject/cancel outcomes, and records from a previous runtime are never
converted into a reusable grant or revived as pending authority. The server persists consent audit
state in `plan_questions`, closes pending rows on restart/disconnect, and uses a single-winner terminal
transition. An `approved` audit row does not change execution mode, mint a grant, or bypass the C01
tool authorization performed immediately before dispatch.

## Approval summaries

Pending mutation approvals persist and display paths, operation, path count, overwrite/recursive
mode, expected version, dry-run/budget values, and a byte estimate. Inline content is never stored
in the approval or audit log. The approval stores a SHA-256 digest of the complete operation, so a
changed operation is a different request. Audit rows contain identifiers, counters, path counts,
and bounded reasons only—not paths, command output, file content, or secrets.
