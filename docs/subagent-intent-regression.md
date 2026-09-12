# Subagent explicit-intent regression audit

## Scope and historical cause

Audited on 2026-09-12 at HEAD `3ed9728cfbe0cb9497e4ee8db7fbaeb0aefb6132`.
The working tree was clean before this repair. No commit, push, live client restart,
or real browser child conversation was performed.

Commit `43e9d71953b7ccda74e531c9ff144d9152802063` (2026-09-10 08:30:28 +07:00),
`fix: prevent unintended ChatGPT child conversations`, introduced the exact
`subagent_explicit_user_intent_required` error and the three defects below.

1. Many command patterns had to start the entire user message. A plugin/project
   wrapper, a later instruction line, or a numbered command such as `Chia 2 agent`
   could therefore hide an explicit delegation request. The old parser also
   admitted descriptive text such as `Chia agent bị lỗi`.
2. `save_user_message` advertised policy based on the current child prompt, while
   `agent_subagent_start` checked the original root task and root turn. This made
   a child's policy instruction contradict the runtime's effective decision.
3. The root query selected the earliest user-message event, including browser
   transcript observations. After transcript correlation, an earlier
   `provider=chatgpt_web` echo could incorrectly veto or grant delegation.

Related historical changes were inspected, not reverted: `2f27e19` added browser
fallback for clients without sampling; `5b4f441`, `16ea611`, and `ba372f6` hardened
conversation identity/routing. The fallback recovery behavior preventing ghost
conversations remains unchanged.

## Repair

- `src/runtime_host/subagent_intent.rs`: clause-scoped command recognition,
  complete-word matching, common Vietnamese/English command and count forms,
  explicit negation, and distinction between delegation and choosing an MCP
  agent or creating an agent settings profile.
- `src/runtime_host/subagent_intent_prose.rs`: preserve instruction boundaries;
  exclude quoted examples, inline/fenced/indented code, blockquotes, and routing
  metadata; normalize common Vietnamese spellings without confusing `đừng`
  with `dùng`.
- `src/runtime_host/user_message_intent.rs`: delegate only subagent recognition
  to the new module. Plan-mode and workflow-hint parsing are unchanged.
- `src/runtime_host/user_message.rs`: exclude browser echoes from the canonical
  root-intent query and reuse that query for both policy reporting and dispatch.
- `subagent_intent_tests.rs`, `subagent_intent_lifecycle_tests.rs`, and test wiring:
  nine new table-based/unit/integration tests covering wrappers, counts, negation,
  examples, root/child/grandchild consistency, transcript authority, per-turn
  isolation, idempotent retry, disabled settings, and contract validation.

This is still a conservative deterministic language recognizer, not an unrestricted
natural-language authorization engine. Merely discussing subagents does not opt in.
The explicit-intent gate, execution approval, path/effect grants, concurrency limits,
lease/heartbeat handling, stop propagation, and browser fallback are not bypassed.
No schema, database migration, tool arguments, frontend, or extension code changed.

## Verification

The new unit tests were run against the original implementation first and reproduced
false negatives and false positives. Separate integration tests reproduced the root
policy and browser-echo defects before the repair.

| Check | Observed result |
| --- | --- |
| Current application tests filtered by `subagent` | 64 passed, including all nine new tests |
| Full Rust workspace, `--no-fail-fast` | 615 passed, 3 failed, 16 ignored |
| All extension `*.test.cjs` files | 190 passed, no failures/skips |
| Full frontend Vitest suite | 222 passed, 14 failed; the run also reported one unhandled error |
| `cargo check --workspace --all-targets --offline --locked` | Passed; an existing Windows-only unused-import warning remains |
| `rustfmt --check` on all seven changed/new Rust files | Passed |
| `git diff --check` | Passed |

The full-repository checks are NOT all green. All three Rust assertion failures
also reproduced in an isolated archive of the original HEAD:

- `user_supplied_absolute_path_grant_persists_for_task`.
- `git_corrupt_repository_and_index_lock_fail_without_panicking`.
- `all_rejects_unstaged_or_untracked_changes_without_mutating_index`.

The baseline archive initially produced one additional native-capture test error
because it did not contain the existing frontend `node_modules`. This is a baseline
setup limitation, not a failure introduced by the repair. The existing dependencies
were subsequently linked for the frontend baseline comparison.

The frontend baseline reproduced the same 14 failed assertions: 13 in
`compactUi.test.tsx` and one layout assertion in `subagentUi.test.tsx`.
The bridge/popup tests passed. These unchanged tests were not weakened or skipped.

`cargo fmt --all -- --check` still reports formatting in unrelated existing files
(for example `src/api/custom_fonts.rs`). Production Clippy with `-D warnings`
reports five existing diagnostics in `src/api/system.rs` and `src/updater/*`;
none are in the changed intent modules. These are not claimed as passing checks.

### Reproducible commands

From the repository root:

```powershell
cargo test -p chat-cmd-client --bin chat-cmd-client subagent --offline --locked
cargo test --workspace --no-fail-fast --offline --locked
cargo check --workspace --all-targets --offline --locked
```

From `web`:

```powershell
node node_modules/vitest/vitest.mjs run --maxWorkers=2
```

From `chatgpt-extension`:

```powershell
$tests = @(Get-ChildItem -File -Filter *.test.cjs | Select-Object -ExpandProperty Name)
node --test --test-reporter=spec @tests
```

Full workspace, baseline, extension, and quality-check logs are in the ignored
`.smoke/subagent-*.log` files. Baseline source is in
`.smoke/subagent-baseline-7bb99654/source`.

### Command evidence

Server-owned `command_run` execution IDs:

- History/blame: `78d00e3a-8b74-408b-9eb2-a7be8966ad47`,
  `8e2ead58-2a3d-4b89-a76b-0fe62fbb0357`.
- Red unit tests: `276dc5db-671f-461e-8b61-be1e9dfeb45f`.
- Final rebuilt 64-test subagent suite: `40e658bc-d37b-441a-9ee1-e019492f91e4`.
- Full current Rust workspace: `831bdcd1-6d4a-4873-8892-53fc07ebaa30`.
- Baseline Rust comparison: `f2e87669-44e7-4844-a76b-48c51b98e9b2`.
- Extension: `f9c69a7b-88fa-48e6-b265-d43b0ee9df36`.
- Current frontend: `87031f8e-f870-4936-be9c-f0723b2cd7f1`.
- Baseline frontend: `acdec32f-da7d-4c7b-b425-dd82f7312c24`.
- Repository quality diagnostics: `bf406c59-dee2-4e1f-9556-0d94649b6abc`.

A shared Cargo target directory temporarily reused the baseline test executable.
That result was rejected: the exact test executable was removed and rebuilt from
this working tree, yielding 64 subagent tests instead of the baseline's 55.
Do not reuse execution `2a8e5265-ef83-49da-9de0-3b46d6b9d461` as evidence for the
new tests. Use separate target directories for future baseline comparisons.

## Activation and remaining limits

These checks exercise isolated test databases and mocked browser/frontend scenarios.
They do not prove a live ChatGPT browser session has loaded the new binary.
Build and restart ChatCMD Client to activate this Rust change. The running client,
real task data, user settings, and current conversations were not restarted or reset.
No claim is made that unrelated baseline test/lint failures or every possible
natural-language phrasing have been resolved.
