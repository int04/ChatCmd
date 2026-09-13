# Subagent startup ACK and initialization diagnostics

## Scope and baseline

Repository: `D:\DEV\CmdGPT\ChatCmdClient`.
Baseline HEAD: `9db77508a99f10fb2eba85ce87e6b8e83ab7c0dc`.
This change follows the missing-finalizer repair; it addresses the earlier startup stage.
The current turn does not commit, push, restart the application, or change live task data.

## Reported incident versus verified evidence

The user reported one browser child timing out and another reporting that its first
`agent_user_message` was blocked, followed by `user_message_sync_required` on finalization.
The latter code means no accepted MCP user message exists for that task/turn. It does
not identify why the first call failed and is not evidence by itself of an OpenAI block.

The diagnostic call recorded in execution `8f0632f4-95ed-4022-86cf-05b0561b7663`
requested `/api/local/overview` and returned HTTP 403 in this tool context.
No authentication bypass or direct live-database workaround was attempted. The original
child tool error and browser logs were therefore not retrieved. The cause of the alleged
upstream safety block remains unverified. No real project YAML file was read as a substitute
for the blocked child. Tests use isolated temporary fixtures.

## Confirmed startup defects

1. `web/src/chatgptBridge.ts` waits five seconds for `subagent-send` acknowledgement, but
   `chatgpt-extension/background.js` previously sent that ACK only after tab loading and
   composer readiness. Those operations can exceed the acknowledgement window. The UI
   could then report failure and schedule a new attempt while the original tab still loaded.
   This behavior was inspected on baseline HEAD before the startup repair.
2. `reportSubagentFailure` previously removed request/binding state and closed the tab
   before the server accepted the failure. It did not fence a late failure against a newer
   attempt, and could destroy a child which had already claimed MCP.
3. Startup checked server state before opening the tab, but not after the potentially slow
   readiness phase. Stop, claim, or retry during that phase could leave a stale prompt send.
4. The prior browser child prompt required finalization for blocked work without first
   distinguishing a failed initialization from work blocked after successful initialization.

Two red-to-green reproductions were recorded before their respective fixes:
- Execution `67dbfe91-e0db-464c-81f1-b51a0249a164`: the lost-ACK UI test failed because
  `reportSubagentFallbackResult` received `status=failed` for an unknown admission outcome.
- Execution `9a4126fa-7586-47c2-a5fd-2dd4e5dd147a`: the sync guard incorrectly accepted
  a browser-only user observation as MCP initialization. The other test, reporting blocked
  work after successful initialization, already passed.
These executions document pre-fix failures, not verification of the final source.

## Changes

### Admission and terminal acknowledgement

`subagent-send` validates the request and acknowledges admission immediately with
`ok=true, accepted=true`. That is only receipt of a startup request, not completion or a
successful file read. Actual asynchronous startup failures use the result endpoint only;
they do not also produce a second UI error response.

The new `background-subagent-failure.js` coalesces simultaneous failure reports for the
same ID/attempt, checks the current binding, and waits for server acceptance before
attempt-fenced cleanup. Missing acknowledgement, a newer attempt, a claimed running child,
or an API transport error does not authorize deleting the live binding or tab.

Startup checks heartbeat state again immediately before prompt submission. A stopped,
claimed, inactive, or superseded attempt must not submit the prompt.

The UI uses the typed `ChatGptBridgeTimeoutError` (`bridge_ack_timeout`). Missing ACK is
an unknown delivery outcome, not an execution failure. It must not advance the child to
another attempt. Explicit rejection still follows normal error reporting. Existing
server leases/deadlines remain the bound for uncertain delivery; no indefinite retries
or automatic execution after a host rejection were added.

### Initialization and finalization

The shared browser prompt and server contract distinguish two states:

- Before `agent_user_message` returns `accepted=true` and `userMessageSynced=true`, do
  not call file/command tools or `agent_turn_complete`. Return an honest blocked report
  with the actual available error. Unknown upstream causes remain unknown. Do not switch
  connectors, disguise requests, or bypass permission/safety denials.
- After successful synchronization, blocked/partial/read-only work can and must report
  its honest outcome through the normal finalizer, subject to active-work checks.

`ensure_user_message_synced` now excludes browser-only transcript observations. Such
observations cannot stand in for an accepted MCP initialization. A diagnostic warning
records task/turn/tool IDs, not private message content, and does not guess the upstream cause.

Public child reports include `mcpUserMessageSynced` and `mcpFinalizerReceived`. These are
based on persisted events and the selected final report source. Browser-only reports
remain `browserFinal` with unknown work/verification when metadata is absent. No browser
report is converted into an invented MCP receipt or verified successful work.

Extension manifest, required-version constant and EN/VI setup copy are aligned to `0.1.17`.
No schema migration, permission expansion, sync-gate removal, or safety override was added.

## Verification

The following executions were directly inspected for the combined final source. A passing
subset does not turn a failing global run into a pass.

| Check | Observed result | Execution ID |
| --- | --- | --- |
| All extension tests | 205 passed, 0 failed | `d7fe2195-936e-4389-b5a9-580dd1054f21` |
| Subagent Rust tests | 78 passed, 0 failed | `9a5626ba-26bb-4db3-ba59-f5ad04d7ca15` |
| MCP crate, coding harness, release catalog | 80 passed, 2 ignored | `843224c6-780c-4ff9-9f86-5621c224e3a6` |
| Full Rust workspace, no fail-fast | 630 passed, 3 failed, 16 ignored; exit 101 | `d02f5d40-dd69-4771-82cc-af2c18ede38f` |
| Full frontend | 229 passed, 14 failed; exit 1 | `3d34d68e-69d7-4564-8ba8-ec3ab6a776e3` |
| Expanded UI subset including popup | 19 assertions passed but runner exit 1: uncaught `root.scrollTo is not a function` in existing popup test | `338c7196-b9af-4ccf-9e0c-1dc6da9bc9aa` |
| ACK/identity/bridge/i18n subset, then TypeScript and production web build | 18 passed; build exit 0; existing bundle-size warning | `4004a48d-8009-45c4-916c-fece4b47abfd` |
| Workspace compilation, all targets | Exit 0; existing Windows unused import warning | `dfccafaa-ab84-411e-97f9-e72e046860eb` |
| Changed-source line limits, rustfmt and diff whitespace | Passed; checked source files at most 500 lines | `e9291457-4bdf-499c-affc-ed09f8b311b8` |

The full frontend failures match the existing 13 Compact UI and one layout assertion
failures reported before this repair. The popup runner error is disclosed separately,
not hidden by the narrower successful run. Source hashes were stable during the final
Rust, MCP, extension, full UI, and successful targeted UI/build executions.

Command logs for the full runs are stored under `.smoke/`:
`subagent-retry-ec2e7fea-workspace.log` and `subagent-retry-ec2e7fea-ui.json`.
No source edits followed those final runs; only this evidence record was updated.

The three Rust failures are the already reproduced baseline failures:
`user_supplied_absolute_path_grant_persists_for_task`,
`git_corrupt_repository_and_index_lock_fail_without_panicking`, and
`all_rejects_unstaged_or_untracked_changes_without_mutating_index`.
The repository must not be described as globally green.

Coverage includes two independent child admissions; delayed ACK; late failure against a
newer attempt; rejected failure after MCP claim; lost API response; duplicate failure
callbacks; stop/claim/retry during tab load; typed bridge timeout and nonce correlation;
unsynchronized child read/finalizer rejection; browser observation not granting sync; and
successful reporting of blocked work after real MCP initialization.

Existing structural extension tests were updated to include the new helper and the final
server-state check between composer readiness and submission. Behavioural tests remain in
place. Related changes appeared concurrently in the worktree; they were read and integrated,
and an overlapping class declaration introduced by this turn was removed rather than
replacing the shared implementation. Final tests run on the combined current source.

## Remaining limits and activation

The original claimed OpenAI safety block is not reproduced or diagnosed by these local
tests. A genuine host denial must remain respected. Its original tool error/trace is needed
to distinguish safety, authentication, tool schema, routing, and transport failures.

No live end-to-end ChatGPT child run was performed after the change. There is no guarantee
that an external model always calls a particular MCP tool. Browser recovery retains its
existing identity/attempt/active-work/stability checks and separate provenance.

The full frontend suite was not rerun in this turn; the affected suites were. Prior global
frontend failures are not declared resolved. The Windows test build retains the existing
unused `ShellCreateRequest` import warning; web build retains the bundle-size warning.

Activation requires rebuilding/restarting the Rust client, using the rebuilt web UI, and
loading extension `0.1.17`. Reload existing ChatGPT/ChatCMD pages only after active work has
finished. With sub-agent concurrency enabled, test a fresh model-selected read-only delegation in
any user language and inspect the actual
`agent_user_message` response and final report receipt fields. An unsynchronized/denied
child should report the blocker, not perform the file read by another route.
