# Subagent regression audit against 3bfb9fc9

> Historical audit. The language-specific explicit-intent gate analyzed here was removed on
> 2026-09-13 in favor of structured enablement through `subagentConcurrency` and model-decided
> delegation. The routing, identity, lifecycle, and observable-error findings remain applicable.

## Scope and reference

Reference: `3bfb9fc95252b2e1ccd3915cb2ee58351b713517` (10 September 2026). Committed HEAD at audit: `9db77508a99f10fb2eba85ce87e6b8e83ab7c0dc`, branch `dev`. The existing uncommitted startup/ACK/receipt changes (extension 0.1.17) were retained. No reset, migration, live-database change, application restart, commit or push was performed.

This audit distinguishes defects reproduced from source from the unverified explanation in another agent's final answer. A model-written claim that a tool was blocked by host safety is not a raw tool error.

## Confirmed regressions

### Request-heading parsing introduced by ebe5c86

`src/runtime_host/subagent_intent.rs::request_body` previously used the last colon and unconditionally discarded a recognized request heading. That could discard the actual delegation command, erase a refusal, or interpret a Windows drive colon as the request separator.

Reproductions:

- `Chia ra agent để thực hiện yêu cầu sau: đọc một file` was refused even though the reference recognizer accepted the leading command.
- `Sử dụng plugin @rust_test để thực hiện yêu cầu sau: chia ra agent đọc D:\DEV\Caplog\Client` was refused because of the drive-letter colon.
- `Không chia agent để thực hiện yêu cầu sau: hãy chia agent đọc file` lost its explicit refusal.
- The standalone phrase `chia ra agent` already worked. It is incorrect to diagnose that exact phrase as universally unsupported.

Before repair, execution `7373334e-5799-4b8b-9af5-f6f9ad64a70e` reproduced three failing tests while the literal-phrase control passed. The repair preserves an existing command/refusal before processing a heading and searches request separators from the left. Existing quote/code/metadata and negation tests remain in force.

### Routed ChatUI input and MCP echo were inconsistent

The routing work in `16ea611` and `ba372f6` preserves task identity even when the model's MCP echo is shortened, provided the request/turn is resolved to the server-owned ChatUI record. `save_user_message` records that echo with server-resolved `bridgeRequestId` and `browserTurnId`.

The intent repair in `ebe5c86` correctly excluded arbitrary browser timeline observations, but then used only the model's echo. It did not account for routed ChatUI's stored original `user_content`. Thus a shortened echo could remove explicit user delegation; conversely an added phrase in the echo could override the original user's refusal.

Execution `764b3cb3-a5f4-46a2-9152-d15ccc143a07` reproduced both errors before repair.

The new `subagent_request_source.rs` uses the original stored user request only after a genuine MCP user-message event has a server-resolved link to the same task, request, browser turn and approved owner. The submitted request must contain its own valid routing footer. It does not use the latest request, arbitrary DOM observations, another task, or an inferred identity. Stale/cross-task links fail closed; legacy unrouted MCP turns retain their existing message behavior. The raw MCP echo remains unchanged for diagnostics.

This does not preinitialize a child on behalf of a rejected tool call. Synchronization, conversation identity, approval and file/budget limits remain mandatory.

## Repair files added/changed in this audit

- `src/runtime_host/subagent_intent.rs`: request-heading parsing.
- `src/runtime_host/subagent_intent_tests.rs`: four regression/control tests.
- `src/runtime_host/subagent_request_source.rs`: fenced original-input resolution.
- `src/runtime_host/subagent_request_source_tests.rs`: ownership, turn, approval and observation guards.
- `src/runtime_host/user_message.rs`: shared root intent decision uses the source resolver.
- `src/runtime_host/identity_delegation_regression_tests.rs`: original/echo regressions and full two-reader runtime lifecycle.
- `src/runtime_host/identity.rs`: test-module wiring only; production routing was not reverted.
- `src/runtime_host/dispatch.rs`: error wording distinguishes local intent validation from host safety. Error code remains `subagent_explicit_user_intent_required`.

Unrelated attachment, conversation-routing and completion/ACK source changes were not reset. No frontend or extension source was changed in this audit.

## Verification on final source

- 89 subagent tests pass, including 11 added tests. `cargo check --workspace --all-targets --offline --locked` also passes. Execution: `5cf3b2c0-444c-4aed-aacb-e6f83f735956`.
- The two-reader test uses actual `call_persisted` synchronization, registration, progress, file read, completion and parent wait with isolated SQLite/temp files. Each child inherits exactly one file and one read call; the legacy read adapter reserves 1 MiB per call, so the fixture reserves 2 MiB total. Both files remain unchanged, both reports are `mcpFinal`, and both MCP finish receipts are true. No live ChatGPT child was opened.
- The negative lifecycle test confirms that a rejected delegation attempt does not prevent progress or finalization of its already synchronized parent turn.
- All 205 extension tests pass. Execution: `ee950245-b963-4546-8db6-69abd66dfa23`.
- Full Rust workspace: 641 passed, 3 failed, 16 ignored. Execution: `2f475fbd-894c-40a2-8543-db839662f2dd`. The same three path/Git assertions previously reproduced on the baseline remain: `user_supplied_absolute_path_grant_persists_for_task`, `git_corrupt_repository_and_index_lock_fail_without_panicking`, and `all_rejects_unstaged_or_untracked_changes_without_mutating_index`. This is NOT a globally green test run.
- Affected UI run: 19 assertions passed, but exit code 1 due to an unhandled `TypeError: root.scrollTo is not a function` in the JSDOM popup test. Execution: `f43bb035-d689-4a33-b7d7-b706b1356b81`. Do not report the UI suite as passed. No retry-to-green or UI production change was used to hide the failure.
- Targeted Rust formatting and source limits were checked; no authored file in this repair exceeds 500 lines.

The first two-reader fixture failed because it lacked a valid inherited read grant, then because it reserved less than the legacy adapter's 1 MiB budget. Those were fixed in the test fixture only; production approval checks were not relaxed.

## Remaining live-test limitation

MCP initialization and progress calls succeeded in this audit conversation. Read-only access to task diagnostics through the normal local-UI endpoint returned HTTP 401 requiring management login. The available in-project log did not contain the latest reported failed call. Therefore the specific claim that progress, finalization and delegation were all blocked in the reported live conversation has not been established or attributed to a platform.

The running executable was observed at `target/debug/chat-cmd-client.exe`, started at 07:40:43 on 13 September 2026, before this audit's edits. The new source requires a normal rebuild and restart to activate. No live end-to-end ChatGPT validation has been performed after the repair. Extension source remains 0.1.17 from the preceding uncommitted repair; this audit does not require a new extension version.
