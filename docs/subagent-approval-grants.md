# Sub-agent approval grants and turn initialization

`approvalGrant` is optional. It reserves a bounded part of an existing, user-approved safe-read grant from the immediate parent. It is not a list of all tools a child needs, a new permission source, or an execution-mode override.

## Choosing tools

Read `subagentPolicy.approvalGrant.allowedTools` from `agent_user_message`. This is an eligibility list generated from the actual catalog, not a claim that these tools have been approved. Names must be distinct and must require approval with a safe-read risk class.

Do not put `git_status`, `git_diff`, `command_run`, or `agent_subagent_start` in this grant. Git tools launch a process and use the ordinary execution approval path. Agent lifecycle tools do not need an inherited safe-read grant. Do not reclassify them to make a request pass.

When no approved parent grant is available, omit `approvalGrant`; actual work still follows the task's normal execution policy, including per-operation user approval. Paths, tools, remaining budgets, expiration, catalog version, parent turn and current worker attempt must all match before an inherited grant is created.

## Initialization and diagnostics

New requests with invalid tool lists or numeric budgets fail in the parent's `agent_subagent_start`, before reserving a child task or dispatching a browser tab. The error names the offending tool when applicable.

A previously stored invalid request, or a grant that expires/revokes/exhausts between registration and claim, must not prevent the child from synchronizing its user turn and reporting a blocker. Grant inheritance runs in a savepoint inside the claim transaction. Policy denial rolls back every grant write and produces `notInherited`; storage failures still fail and roll back the claim. Nothing changes the task execution mode, tool authorization, allowed paths, or approval workflow.

The result is durable in a system timeline event bound to the sub-agent ID and worker attempt. It is exposed as:

- `agent_user_message.subagentApproval` for the child.
- `agent_subagent_wait.subagents[].approvalGrant` for the parent and root, including grandchildren.
- `TaskDetail.subagents[].approvalGrant` in the local API.

States are `inherited` (with `grantId`), `notInherited` (with `errorCode`, message and next-step guidance), `notRequested`, and `unknown` for legacy records without a diagnostic. These fields report inheritance, not general permission to execute and not proof of successful work. A failed optional grant cannot satisfy an execution approval.

Duplicate claims do not mint another grant or reserve parent budget again. Grants from an old parent attempt cannot be re-delegated. Grandchild reservations debit the immediate parent's already-reserved allowance rather than debiting the root a second time. Parent constraints and expiry are retained.

## Regression coverage

Runtime tests cover invalid/duplicate tools before child creation, a legacy invalid grant with successful turn synchronization but an unapproved read timing out, root deny for reads/writes/Git/process tools, valid parent-child-grandchild reads with two global slots, duplicate synchronization, exact report propagation, concurrent reservation without overspending, revoked/expired/exhausted/wrong-turn/stale-catalog/stale-attempt parent grants, and database-error rollback. All tests use isolated temporary repositories and approval state, not live user tasks.
