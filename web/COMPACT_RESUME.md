# Compact & Resume — React integration

## Ownership

This implementation is scoped to `web/`. Rust persistence/checkpoints and the browser extension runner are implemented independently. No live ChatGPT tab or user database is used by the UI tests.

## Entry points

`TasksPage.tsx` mounts one task-scoped `CompactProvider` around the existing chat and sidebar. `ChatGptTaskComposer.tsx` hosts `CompactAction` and `CompactStatusCard`; `CompactHistoryCard` sits immediately after `TaskAccessCard`.

`CompactSession.ts` owns server state, task validation, revision-aware merging, polling and extension recovery. The composer and history share one snapshot, avoiding duplicate pollers. API calls remain centralized in `api.ts` and use ordinary JSON; no application-layer crypto session is created.

## Backend contract consumed

| Method | Local route | Result/body |
| --- | --- | --- |
| GET | `/api/local/tasks/{taskId}/chatgpt/compact` | `{ active: CompactJob|null, history: CompactJob[] }` |
| POST | `/api/local/tasks/{taskId}/chatgpt/compact` | Body `{ continueAfterCompact: boolean }` (defaults to false); returns the new or existing nonterminal `CompactJob` |
| GET | `/api/local/chatgpt/compact/{jobId}` | `CompactJob` |
| POST | `/api/local/chatgpt/compact/{jobId}/checkpoint` | Body `{ expectedRevision, phase: 'cancelled' }`; returns updated job |

Types live in `src/chatgpt/compact/types.ts`; `CompactJob.continueAfterCompact` records the immutable choice from start. The confirmation checkbox is unchecked each time the dialog opens. Cancelling never starts a job or persists a choice. Terminal phases are `completed` and `cancelled`. Cancellation conflicts trigger a read and visible feedback, not an automatic retry with a newer revision.

`CompactStatusCard` renders only an active job, never the most recent history entry. On completion/cancellation it disappears, including after a reload; load errors use a separate lightweight alert with Retry.

On completion, the UI refreshes the existing ChatGPT bridge record in place. It does not create a task, alter its route, or remount the composer/queue. Sending remains paused until the bridge conversation ID matches the completed job's new ID (URL fallback when necessary). Drafts remain in their existing React state; queued messages remain in the existing backend queue. This does not introduce draft persistence across a full browser reload.

## Extension integration required

The existing `chatgptBridge.ts` envelope sends:

```ts
{
  type: 'chatcmd-chatgpt-extension-request',
  action: 'compact-resume',
  nonce,
  jobId,
  taskId,
  localBaseUrl: window.location.origin
}
```

The parent extension implementation must add `compact-resume` to the `content-chatcmd` forwarding allowlist/dispatch and return the existing nonce-correlated `chatcmd-chatgpt-extension-response` envelope. This UI does not edit the extension. The runner must treat repeated wake requests as idempotent recovery of the same durable job, honor cancellation/checkpoint revisions, and refresh its persisted state before continuing.

The backend/runner must also prevent consumption of existing immediate/queued messages during handoff, and publish the new bridge binding before or atomically with completion. React pauses its own sends and queue edits but cannot prevent another tab or extension process from consuming messages. Opening a history reference must not rebind the old ChatGPT conversation to the active task.

## Recovery and polling

Mount, reconnect, online/focus/visibility return, and an unavailable-to-ready extension transition trigger recovery reads. Active jobs poll every 2 seconds; inactive history refreshes every 15 seconds. `chatgpt_compact_updated` events refresh matching tasks. Wake requests are coalesced and throttled to one per job per 10 seconds; ordinary active polling does not continuously relaunch the extension.

A missing extension leaves the durable job intact and exposes an actionable message, resume button and cancellation button. A lost POST response triggers a new authoritative read and keeps sends locked while the outcome is unknown. Stale lower-revision poll responses cannot resurrect a cancelled/completed job.

History is sorted newest-first and deduplicated by job ID. Old conversation links are plain HTTPS ChatGPT anchors using `_blank` and `noopener noreferrer`, with no task navigation or bridge action.

## Verification

From `web/`:

```text
npm test -- --run src/test/compactSession.test.ts src/test/compactUi.test.tsx src/test/compactContract.test.ts src/test/chatGptIdentitySync.test.tsx
npm test -- --run
npm run lint
npm run build
```

The compact tests use mocked local APIs and a mocked extension boundary. They cover exact confirmation/cancel and keyboard dismissal, unchecked-by-default continuation and explicit opt-in, terminal-panel removal, stage status, sidebar position/old links, route preservation, polling/reload/reconnect, missing extension, CAS/stale responses, unknown POST outcomes, draft/queue preservation and sending against the new bridge URL. Live end-to-end backend/extension validation remains a separate integration step.
