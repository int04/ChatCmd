# Tool resource budgets

Long-running runtime work uses `ToolBudget` and `BudgetTracker`. Effective limits are the
intersection of server hard caps, policy/default caps, and caller requests. `None` means that a
layer adds no restriction; it never removes a restriction imposed by another layer. A zero
timeout is either rejected or treated as an immediate timeout by the legacy schema; it is never
infinite. Deadlines use monotonic `Instant` values.

## Budget matrix

| Tool family | Default timeout | Hard timeout | Principal hard caps | Partial/cancellation semantics |
|---|---:|---:|---|---|
| `fs_find_v2` | 10 s | 60 s | 1,000,000 entries; 100,000 metadata calls | Bounded partial page and cursor |
| `fs_search_v2` | 15 s | 60 s | 1,000,000 files; 2 GiB read; 4 MiB result; 256 MiB/file | Bounded partial page and cursor |
| `fs_read_text_v2` | 10 s | 60 s | 128 MiB read; 4 MiB result | Bounded range with continuation offset |
| `fs_apply_edits` | 15 s | 5 min | 2 GiB read/write; 10,000 edits | Staging is rolled back before returning; a published result wins a late cancel |
| copy/move/delete | 5 min | 30 min | 2,000,000 entries; 2 TiB read/write | Pre-publish rollback; post-publish state is reported explicitly |
| Git subprocess | 30 s | 10 min | 4 MiB stdout preview; 1 MiB stderr preview; 1 GiB artifact | Process tree is killed and child reaped on cancel/timeout |

## Architecture

- `BudgetTracker` is cloneable and shared with blocking workers. Checkpoints test cancellation and
  monotonic deadlines; atomic counters reject boundary overflow without counter overshoot.
- `AdmissionController` has weighted global permits, per-actor permits, and checked memory
  reservations. Admission is immediate and returns retryable `admissionDenied`; RAII releases all
  reservations on normal return, error, cancellation, or panic unwind.
- Search/find/copy/move/delete enter shared admission control. Search progress is rate- and
  count-limited, coalesces intermediate updates, and always sends one final counter update.
- Process output uses fixed chunks and bounded signalling. Runtime and output/artifact requests are
  clamped to hard caps before spawning the process.
- Persisted dispatch keeps polling a cancelled worker until cooperative cleanup completes. A
  successful post-commit result is retained instead of being misreported as stopped.

## Result and error contract

Budget failures use stable codes: `operationCancelled`, `timeBudgetExceeded`,
`fileBudgetExceeded`, `entryBudgetExceeded`, `byteBudgetExceeded`, `outputBudgetExceeded`,
`progressBudgetExceeded`, and retryable `admissionDenied`. Tracker errors include phase and a
serialized `BudgetUsage` snapshot. Search and find preserve their existing truncation/cursor
envelope for partial reads.

## Migration status

Migrated in this change: search, find, ranged text reads, range edits, recursive safe mutations,
Git process execution, and persisted cancellation waiting. Blob storage retains its existing
upload/download quotas and ownership checks; interactive PTY sessions retain their session and
replay limits. Converting those two legacy subsystems to emit `BudgetUsage` directly remains a
follow-up because it changes their public response schemas.

## Operational diagnostics

Budget errors include the phase and consumed counters. Admission errors are retryable and should
be counted by tool and actor. Progress events contain paths and counters only; they never include
file content. CI tests assert cap precedence, exact boundaries, cancellation usage, permit release,
bounded progress, process reap, mutation rollback, and traversal continuation behavior.
