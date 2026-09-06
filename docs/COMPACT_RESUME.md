# Compact & resume now

## Usage and deployment

Build the web assets with `cd web && npm run build`, then build/run the updated Rust application from the repository root (`cargo run` for local development). Restart an already running application with the updated binary; changing source does not upgrade a running backend. Load/reload the unpacked `chatgpt-extension` directory in Chrome/Edge. Extension version 0.1.9 uses compact content protocol 3; the `alarms` permission was introduced in 0.1.7. Existing tabs are checked for that protocol and reinjected when necessary; a manual page refresh is also supported.

On `/tasks/task-chat-{id}`, choose **Compact & resume now** next to the existing conversation actions. Nothing is dispatched before confirmation. The checkbox **Tiếp tục công việc sau khi compact xong** starts unchecked on every opening. Leave it unchecked to transfer the handoff and attach the new ChatGPT chat without a working follow-up; check it to send the continuation request after attachment. Both ChatCMD and the source ChatGPT composer display **ChatGPT is writing the handoff**, with Preparing, Writing the handoff, Saving it and Opening the new chat. The source panel also receives the later saving/opening checkpoints. The destination panel appears above its composer once loaded. Progress panels disappear when the job completes or is cancelled; completed entries remain only in the sidebar history.

The sidebar's **Lịch sử thu gọn ngữ cảnh**, directly below execution permissions, contains timestamps, states, model and old/new conversation identifiers. Its reference links always open the OLD ChatGPT conversation in another tab; viewing history never makes that archived conversation the active task binding. Pending progress also offers links to reopen the known source/destination conversations.

## Reference behavior

The implementation was studied against these files in `D:\DEV\chat-on-steroids` (read only):

- `src/main/session/continuation.ts`
- `src/main/session/handoff.ts` and `handoff-prompt.ts`
- `extension/content.js`, including its compact handoff and destination-resume controller

The retained principles are exact public-answer ownership, a durable handoff before opening another chat, dispatch fences before irreversible Send, and destination acknowledgement before changing the application's conversation binding. ChatCMD uses its own Rust/SQLite and extension bridge rather than the reference application's session/file layout.

## Handoff detail and compatibility

Version 0.1.9 expands `compact-protocol.js::handoffPrompt` into section-specific instructions. The reference `src/main/session/handoff-prompt.ts::HANDOFF_BRIEF_RULES` emphasizes every material user correction, verified-versus-planned state, causal bug/fix/evidence chains, delegated ownership and environment/install details. Those requirements are now explicit in ChatCMD too, with twelve operational sections, per-requirement status, exact command/executionId references, stale/unverified evidence and a final completeness check. It retains the existing character budget rather than adopting the reference's 10,000–30,000-token target for substantial sessions: useful detail is prioritized within the 70,000-character requested answer budget, without padding short sessions.

Prompt length is not transferred context length. The replacement chat receives the actual saved handoff answer inside `resumePrompt`, not a replay of the entire old conversation. Neither a longer prompt nor a valid end marker proves semantic completeness. The generator can omit facts or have already lost access to earlier context; the instructions require those gaps to be acknowledged. Automated tests check protocol/ownership/transfer behaviour and the presence of instruction requirements, not whether an LLM remembers every fact. The original conversation remains available through sidebar history even after its tab closes.

`handoffPrompt(job, 1)` keeps the pre-0.1.9 prompt byte-identical. The content controller accepts either complete known prompt for the same job, never just a marker prefix. This lets already-sent old jobs finish, and sends an already-staged old prompt without replacing it. New requests use the detailed v2 prompt. Unknown altered prompts or multiple same-job marked turns still fail closed. No new SQLite migration or backend contract is needed for this update.

## Durable state and same-task identity

Migration `0023_chatgpt_compact.sql` adds jobs, old-conversation archives, operation records and obsolete-request fences. Migration `0024_compact_continue_opt_in.sql` adds the per-job `continue_after_compact` boolean, defaulting to false for all existing and new jobs. A repeated start returns the initial choice unchanged. Neither migration creates replacement tasks. A partial unique index permits only one active compact job per task. Checkpoints compare `expectedRevision`; stale writers receive HTTP 409. Phase transitions are monotonic. The complete handoff must have been saved in a previous successful checkpoint before opening or committing the destination.

Completion is one SQLite transaction: archive the old URL/ID/scope/session/request metadata, retire old bridge requests, update `chatgpt_conversations`, change the existing task's authenticated OpenAI conversation scope, and clear obsolete active session/request bindings. Task ID, title, project, permissions and accumulated timeline stay on the original task. Callbacks and native capture from archived chats cannot create a recorder task or overwrite the new binding. MCP admission checks precede identity creation, and authenticated conversation scope takes precedence over a possibly reused transport session.

Preparing fences new local MCP operations for the compacting task. Calls already admitted are tracked until they finish; advancing to handoff waits for this barrier. Unknown/unavailable local activity is not interpreted as zero. The current ChatGPT generation is stopped before composing the handoff. User drafts are not overwritten, including a draft entered while model selection is awaiting completion.

The first message in the destination is a no-tools bootstrap acknowledgement. Only after its real canonical URL and marker are observed does ChatCMD commit the new binding. Only when the saved `continueAfterCompact` choice is true does a deterministic `compact-continue-{jobId}` bridge request continue work on the same task. The extension skips `/resume` when it is false or absent, and the Rust endpoint independently enforces the stored choice, including calls from older extensions. Repeating the resume API does not enqueue another working message; a real already-created user request wins that race. Existing drafts and queued messages are retained.

## Tab closure, reload and uncertain dispatch

After the handoff is saved, the new chat is identified, and the completed same-task attachment is committed, `finishCompactBrowser` calls `retireCompactSource`. It closes only the recorded source ChatGPT tab, regardless of the continuation checkbox. Merely creating a blank destination is not enough. Closing the browser tab does not delete the ChatGPT conversation or its archived URL. If the source is selected in the same window, the destination is selected before removal; a background source does not take focus from ChatCMD.

Retirement checks the current source URL, pending navigation, exact handoff user message, document token, lack of a new user turn/draft, and idle generation. It durably records the closing document and rechecks before removal. A changed/reused tab, new draft, new user turn, ambiguous marker or unproven source is left open. It does not scan all old-URL tabs or close history copies. Transient removal errors keep browser cleanup recoverable without repeating the compact or blocking opted-in work. A reused/reloaded document after uncertain removal is not closed. Already-finished jobs are not retroactively retired on upgrade, so browsing an old link later never triggers closure.

SQLite owns the handoff and semantic phase. Extension local storage owns browser tab metadata and source/destination Send fences. The service worker uses bounded polling while active, an alarm, startup, tab updates and content wakeups to reconcile jobs. There is no short timeout that discards a compact simply because the user closes a tab and returns later.

| Situation | Recovery |
| --- | --- |
| Source closes before or during writing | Keep the job. Reopen the source URL; capture the exact marked public answer when available, without submitting the same request again. |
| Handoff has been saved | Do not ask the source to write it again. Continue from the saved SQLite checkpoint. |
| Destination closes after receiving the bootstrap | Restore/open that destination conversation. Its exact RESUME prompt and canonical URL identify it, even if its tab ID or URL fragment changed. |
| Destination closes while still a blank draft | Restore the actual closed tab (for example, Chrome/Edge's reopen-closed-tab action). The operation fragment and dispatch record restore ownership. An unrelated blank tab is not adopted automatically. |
| Extension or page is reloaded | Reload jobs from SQLite and dispatch metadata from extension storage; stale document tokens cannot click Send. |
| Send is not yet enabled | Keep the staged prompt and poll readiness without rewriting it. Read contenteditable paragraph/line-break boundaries rather than concatenating textContent. |
| Send becomes unavailable after preparation | Only an explicit no-click acknowledgement for the same document token permits a durable `not-sent` state and retry. Exceptions or lost replies never count as this proof. |
| A Send/checkpoint response is lost | Observe the existing marked user message and durable revision before proceeding. No blind duplicate Send. |
| Dispatch ownership cannot be established | Keep the job paused with a visible explanation. Restore the exact chat/tab, use the resume control, or explicitly cancel and inspect the conversation. Missing acknowledgement is not reported as success. |

An unresolved dispatch already stored by version 0.1.7 cannot be retroactively classified as not-sent. Inspect the original ChatGPT tab and either let its existing marked message finish or cancel the old job before starting another; never clear its dispatch fence merely because no response is currently visible. A job still in Preparing with its matching draft can proceed using the corrected composer reader.

If ChatGPT refuses to generate a handoff (for example, a server-side restriction on further messages), the feature cannot manufacture a verified summary. It preserves the original conversation and reports the blocked/waiting state instead of opening an empty replacement or silently truncating context. No hidden reasoning, tool-result DOM, invisible descendants or unrelated assistant turn is used as a fallback handoff.

## API and code map

- `GET/POST /api/local/tasks/{taskId}/chatgpt/compact`: history/current job and idempotent start; POST accepts `{continueAfterCompact: boolean}` with omitted choice defaulting to false.
- `GET /api/local/chatgpt/compact/pending`, `GET /api/local/chatgpt/compact/{jobId}`: extension recovery and full saved handoff.
- `POST .../{jobId}/checkpoint`: revision-checked progress and atomic completion.
- `POST .../{jobId}/resume`: idempotent post-attachment working request.

UI routes retain local GUI authentication/session handling; extension routes are explicitly allowlisted, not a blanket management bypass. List/history responses omit the handoff body. Realtime `chatgpt_compact_updated` notifications are hints; polling and database revision remain authoritative.

Main modules: `crates/chatcmd-storage/src/compact/`, `src/api/chatgpt_compact*.rs`, `src/runtime_host/compact.rs`, `web/src/chatgpt/compact/`, `chatgpt-extension/compact-protocol.js`, `content-chatgpt-compact.js`, `background-compact.js` and `background-compact-destination.js`.

## Verification

Run the actual regression suites:

```text
cargo test --workspace --no-fail-fast
cd chatgpt-extension
node --test
cd ../web
npm test -- --run
npm run build
npm run lint
```

Rust tests cover transaction/revision rules, restart persistence, preserved task identity and permissions, archived callbacks, auth boundaries, queue preservation, local-tool draining and idempotent working resume. Extension tests execute the production controller/protocol/DOM parser and worker modules, including combined content/worker restart with lost responses, concurrent tasks, draft races, exact marker ownership and hidden-content exclusion. Additional regression tests reproduce multi-paragraph contenteditable input, delayed Send enabling, explicit no-click retry across reload, lost real-click replies, checkbox opt-in/off, terminal-panel hiding, and schema-23-to-24 upgrade with no invented consent. The Rust native-capture integration runs shipped scripts against an isolated real HTTP router and SQLite database with synthetic Chrome/DOM boundaries.

These automated tests do not establish that the current live ChatGPT DOM/account restrictions work in every browser. A live-account smoke test should cover a normal compact, closing/restoring each tab at every stage, a delayed return, extension reload, and two simultaneous tasks. Do not test against an unrelated in-progress conversation or reset the user's normal database.
