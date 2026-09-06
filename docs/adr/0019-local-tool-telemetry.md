# ADR 0019: Local tool telemetry and bounded diagnostics

## Status

Accepted.

## Decision

ChatCMD records tool health and resource telemetry through one `ToolTelemetryRegistry` facade.
The runtime emits a `chatcmd.tool_call` span and one terminal event at the persisted-call boundary.
The application keeps aggregate counters and a bounded terminal history in memory and exposes a
bounded snapshot at authenticated local endpoint `GET /api/local/diagnostics/tools`.

Telemetry is local-only by default. No remote exporter or subscriber is installed by the runtime
library. A future OpenTelemetry or Prometheus exporter must consume the same facade snapshot and
must preserve the label and redaction rules below. Set `CHATCMD_TELEMETRY=off` (also `0` or
`false`) before starting the application to disable spans, events, active tracking, and metrics.

## Taxonomy

The span contains only the finite `tool.name`, typed `tool.class`, hashed request/task/turn/session
correlations, phase, terminal status, allowlisted error category, and numeric usage. Phases cover
authorization, scans/reads, mutation staging and rollback, processes, artifacts, approval,
subagents, synchronization, and cleanup.

Metrics are grouped by `(tool, class, status)`. All three labels have finite allowlists; unknown
tool names collapse to `unknown`. Each group tracks calls, total/max duration, queue wait,
fixed duration/queue histograms (1/5/10/50/100/500/1000 ms/overflow), files/entries scanned,
bytes read/written/output, progress, retries, truncations, and artifact externalizations with
saturating arithmetic. Diagnostics also expose bounded active operations, progress outcomes, and
artifact/blob/journal resource gauges. Correlation IDs never become metric labels.

`ToolUsage` is the small result-envelope view. It includes elapsed and queue time, optional
files/entries/bytes, output bytes, progress events, and retries. `BudgetUsage` converts directly to
this type so enforcement and reporting use the same counters rather than parallel accounting.

## Data classification and redaction

- Allowed: finite catalog key/class/status/phase/error category and numeric resource counts.
- Correlation only: request, task, turn, and logical session IDs are SHA-256 hashed and truncated.
- Forbidden: arguments, command strings, file paths/content, Base64, environment values, tokens,
  authorization/password/secret/key values, agent IDs, raw errors, and conversation scope.
- Error messages remain at their existing user/timeline boundary; telemetry stores only an
  allowlisted error category. Unknown values collapse to `internal`.

Terminal metrics are never sampled. In-memory history retains at most 1,024 terminal records and
completed request deduplication retains at most 4,096 IDs. Diagnostics return at most 256 active
records and report omitted counts. Mutex poisoning or disabled telemetry silently degrades
observability and cannot fail a tool call.

## Validation

Regression tests cover all terminal statuses, exact usage aggregation, deduplication, bounded
snapshots, correlation/content redaction, finite unknown labels, overflow saturation, progress
outcomes, cleanup, and telemetry-backend failure. The `tool_telemetry` Criterion benchmark compares
10,000-call batches with telemetry disabled and enabled.

On the Plan 19 Windows validation host, the final release benchmark measured a 10,000-call batch at
1.32–1.39 ms disabled and 5.81–5.97 ms enabled (about 0.45 microseconds incremental cost per call,
1.68–1.72 million instrumented calls/second). Four contending threads completed the same batch in
11.58–12.11 ms (826–863 thousand calls/second). These numbers describe this host only; they are not
a cross-platform latency guarantee. End-to-end timings include correlation and registry
allocations; bounded history/dedup structures prevent growth with total process lifetime.
