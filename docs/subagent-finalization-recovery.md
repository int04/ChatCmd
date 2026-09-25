# Browser subagent: missing MCP finalization recovery

Date: 2026-09-12. Base commit: ebe5c86 (restore explicit subagent delegation intent).

## Reproduced failure

A browser child can synchronize agent_user_message, use MCP tools successfully, then finish its public ChatGPT answer without calling agent_turn_complete. The child is already running/claimed. Previously, subagent_fallback_result rejected every browser result in that state as already_claimed_or_finished. The extension continued polling, while browser heartbeats renewed the lease. The generic finalization watchdog intentionally skips an active child, so only the hard deadline eventually released it.

This state was reproduced by an isolated database test before changing production code. Execution c60e1e3c-4277-4f1d-a265-9ff9c36b435c failed at the expected old rejection. This is a reproduced protocol gap, not a claim that the original failing child's live browser/logs were inspected.

Additional inspected gaps: browser subagents were excluded from the request-scoped transcript observer; result delivery could mark an HTTP-successful but rejected result as reported; completion cleanup did not use the existing attempt fence; and a fast MCP claim could prevent a later started callback from persisting its conversation identity.

## Changes

### Required browser MCP lifecycle

src/runtime_host/subagent_fallback.rs builds the same compact browser-child route for initial dispatch and retries. The prompt preserves one CMDGPT_SUBAGENT_ID marker and identifies the reserved child task and turn. The child synchronizes by sending only that marker to agent_user_message; the server resolves the stored delegated request. After synchronization it completes the assigned calls and uses agent_turn_complete before its public answer. The parent owns skill discovery, so a bounded child does not repeat skills_list unless its objective specifically concerns skills.

The sampling runtime owns synchronization, heartbeat and finalization. Lifecycle tools are removed from the sampled child's catalog, while skill discovery remains available when required context was not supplied. Parent finalization metadata is removed from task-tool results shown to that child without removing tool-owned fields such as shell_wait.completed. No new permission grant, execution mode, child delegation allowance or database migration is added.

As of the 2026-09-13 follow-up, a browser answer produced before MCP user-message synchronization is no longer accepted as a completed child report. It retries the same fenced child through the configured attempts and ends failed/exhausted if none claims MCP. Browser prose cannot establish that a tool ran or failed; runtime call/result evidence remains authoritative.

### Browser fallback, not a counterfeit MCP message

The extension now uses a local-only request-scoped transcript observer for browser children. It keeps user/assistant message identities, excludes tool bodies and commentary, stops tracking when the user turn or conversation changes, and retains unacknowledged checkpoints. It does not send child snapshots to the parent-chat observation API.

The child monitor requires 12 seconds of stable answer text with no generation/stop indicator before sending protocol-1 completion evidence. It rechecks the current observer before transmission. The Rust endpoint requires the correct conversation and fallback attempt, a single canonical delegated user turn, a live parent/child within the hard deadline, and no active tool or descendant. A server-owned candidate digest binds the exact report, DOM identities, worker attempt and latest activity. The same candidate must survive an additional 12-second server grace window; changed content or activity resets it, and active work clears it.

A successful recovery stores the browser report, completes the run/task and revokes remaining child grants in one SQLite BEGIN IMMEDIATE transaction. A failed report write rolls everything back. Duplicate/concurrent callbacks cannot create another recovered report. Native completion, cancellation, stale attempts, deadline expiry and ambiguous later turns are not overridden.

The result is explicitly completionSource=browserFinal and mcpFinalizerReceived=false. The stored event has provider=chatgpt_web, finalizerMissing=true and recoveredFromBrowser=true. Its report retains workOutcome=unknown and verification=unknown when no corresponding native quality report exists. Lifecycle completion is not verified work. The parent must review the returned report normally.

### Acknowledgement, identity and deployment

The extension marks resultReported only after a real completion/terminal acknowledgement. Missing acknowledgements, rejected active states and stale attempts keep the request pending. Successful cleanup uses closeSubagentRequest with the original attempt, rather than unconditionally removing a potentially newer child's binding/tab.

A late started callback after MCP claim can now persist matching conversation identity without reverting the child to pending/started, changing its worker attempt or extending/reopening its lifecycle. Existing conversation-binding guards remain in place.

The extension version and the web UI's required version/help text are 0.1.16. Apply the Rust build and reload the updated extension plus relevant pages after active work has finished. This change has not restarted the running application, reset its data, modified live child rows, committed or pushed anything.

## Verification

- Current subagent suite: 74 passed, 0 failed. Execution 6a5ce580-4e6e-4200-9d08-145eb9c87e72 also passed cargo check --workspace --all-targets --offline --locked.
- All extension tests: 197 passed, 0 failed. Execution c903aed4-ea0c-4572-895f-056ad48ae8d1.
- UI subagent bridge/popup, ChatGPT identity sync and i18n: 12 passed. TypeScript compilation and Vite production build passed; Vite retains a bundle-size warning. Execution 756c708e-3743-4d27-a74e-e8f4c802a03a. Its web-directory source snapshot hit the byte budget, so this is not a complete source-hash attestation.
- Full Rust workspace: 625 passed, 3 failed, 16 ignored. Execution c52215ca-f7ac-486d-9d30-4e1d31408311. The failures are the previously reproduced baseline assertions: user_supplied_absolute_path_grant_persists_for_task, git_corrupt_repository_and_index_lock_fail_without_panicking, and all_rejects_unstaged_or_untracked_changes_without_mutating_index. Do not describe the whole repository as green.
- Existing Windows test warning: unused ShellCreateRequest import. No new warning was observed in cargo check/test.

New Rust tests cover recovery and report provenance, missing/invalid evidence, wrong identities and stale attempts, active tools/descendants, changed answers/activity, ambiguous later turns, terminal races, transactional failure, concurrent callbacks, late started identity and grant revocation. Extension tests exercise the actual observer/monitor and acknowledgement handlers, including two independently monitored children, role-less final Markdown, excluded commentary/tool text, checkpoint acknowledgement, no premature cleanup and attempt-fenced closing.

Full logs are in .smoke/subagent-finalization-workspace.log, .smoke/subagent-finalization-extension.log and .smoke/subagent-finalization-web-build.log. The earlier full frontend suite was not rerun for this change; the targeted affected suites and production build were run.

## Boundaries

A prompt cannot guarantee that an externally hosted model will make a specific MCP call in every response. This repair reinforces the required call and removes the reproduced stuck-state dependency on that call; it does not claim that browser recovery is an actual MCP finish. Recovery still needs an available browser observer, valid identities, an accessible local API and qualifying final-answer evidence. Browser/process crashes, missing DOM evidence, active work, approval waits or later user turns are not silently declared successful; existing deadlines and stop controls remain authoritative. No live end-to-end child conversation was opened after installing the change in this session. Automated tests use isolated databases and browser mocks.
