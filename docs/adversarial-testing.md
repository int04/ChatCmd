# Adversarial testing and benchmarks

Plan 23 adds generated fixtures, resource probes, deterministic concurrency gates, a real
subprocess kill harness, packaged MCP transport coverage, Criterion benchmarks, and three CI
tiers. Fixtures live only in unique temporary directories; no generated binary is committed.

## Test tiers

| Tier | Trigger | Scope |
| --- | --- | --- |
| 0 | Every pull request and `dev` push | Format, all-target check, complete workspace test suite |
| 1 | Nightly/manual, three operating systems | 10 MiB streaming file, sparse 1 GiB ranges, deterministic writer race, bounded tree search, Git spill, crash harness, packaged MCP catalog |
| 2 | Nightly/manual, Linux | Criterion reports for middle-range reads at 1/10 MiB, a 1,000-file search, 1 MiB atomic replacement, and telemetry |

Run the tiers locally:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo test -p chatcmd-runtime --test adversarial_filesystem
cargo test -p chatcmd-mcp --test release_catalog_smoke
cargo bench -p chatcmd-runtime --bench filesystem_workloads -- --sample-size 10 --measurement-time 1 --warm-up-time 1
```

Criterion writes HTML and machine-readable measurement data below `target/criterion`. Compare
medians and distributions from the same machine. A 5–10% timing change is informational; hard
correctness limits such as bytes read, output bytes, and files scanned remain test assertions.

Initial Windows baseline (2026-09-04, 10 samples, one-second warmup/measurement):

| Case | Median |
| --- | ---: |
| 1 MiB file / 4 KiB middle range | 384.55 us |
| 10 MiB file / 4 KiB middle range | 388.99 us |
| Search 1,000 files, no match | 26.192 ms |
| Atomic replace, 1 MiB | 9.6698 ms |
| Telemetry disabled, 10,000 calls | 1.3429 ms |
| Telemetry enabled, 10,000 calls | 5.9563 ms |
| Telemetry enabled, 10,000 calls / 4 threads | 11.643 ms |

These figures establish order of magnitude, not portable pass/fail thresholds. Retain Criterion
reports as CI artifacts and compare only equivalent machine/configuration runs.

## Fixture and harness contracts

- `support/fixtures.rs` writes deterministic pseudo-random files in 64 KiB blocks, calculates a
  streaming SHA-256, supports sparse files, and places known markers at the beginning, middle,
  and end. The tree generator creates configurable file counts/depth plus ignored content.
- `support/resource_probe.rs` records bytes, entries, and maximum buffered bytes with atomics.
- `support/fault_injection.rs` names mutation commit phases and provides channel-controlled gates.
  Gates use messages rather than sleeps, and are test-only helpers—not tool/API inputs.
- `support/process_helper.rs` starts the current test binary, waits for a named stdout phase,
  kills it, and always reaps it before returning.
- `adversarial_filesystem.rs` launches its own test executable as a helper, waits for a marker
  emitted after staging, kills and reaps it, then proves the old target stayed complete. Every
  test uses a `tempfile` root and explicitly cleans retained staging data.

Log the random seed, fixture size, fault point, OS, Rust version, CPU, and Criterion command when
reporting a failure. Do not retry failed assertions. A bounded retry is allowed only for Windows
temporary-directory cleanup after a sharing violation.

## Traceability matrix

| Plan | Principal direct evidence after Plan 23 |
| --- | --- |
| 01 | `chatcmd-mcp/tests/release_catalog_smoke.rs`: packaged process and real MCP transport |
| 02 | `tool_result.rs` unit tests: bounded unified result envelope |
| 03 | `read_text_streaming.rs`; adversarial 10 MiB and sparse 1 GiB range tests |
| 04 | `filesystem_list.rs` cursor, mutation, and budget tests |
| 05 | `filesystem_find_tests.rs` traversal/cursor/budget tests |
| 06 | `search_v2.rs`; generated-tree file-budget test and search benchmark |
| 07 | traversal denial, symlink/path tests, and ignored subtree in generated fixture |
| 08 | `file_version.rs` replacement, forgery, cancellation, and schema tests |
| 09 | `direct_runtime.rs`; simultaneous expected-version writer test |
| 10 | `blob_store.rs` chunk ownership, quota, hash, expiration, and cleanup tests |
| 11 | `atomic_write.rs`; real staged-process kill test and atomic replacement benchmark |
| 12 | `mutation_safety.rs` verified publish, cancellation, overlap, quarantine, and cleanup |
| 13 | `tool_result.rs` and storage tests for bounded/redacted persistence |
| 14 | turn file-change tracker tests in the application runtime host |
| 15 | Git process-runner tests plus large-status inline/artifact cap test |
| 16 | `budget.rs`, process-runner cancellation, read/search resource assertions |
| 17 | request identity and packaged catalog transport tests |
| 18 | `subagent_worker_tests.rs` lease, stale worker, timeout, restart, and fallback tests |
| 19 | `telemetry.rs`, `tool_telemetry` benchmark, and atomic resource probe |
| 20 | `repository_index_batch.rs` batch caps, isolation, and stale index behavior |
| 21 | `policy.rs` and storage approval-grant concurrency/scope tests |
| 22 | shell unit/integration tests for bulk input, coalescing, exit, and bounded replay |

## Manual and platform-only matrix

The following remain Tier 2/manual because they need long runtimes, privileges, special mounts,
or external resource controls:

- dense 100 MiB/1 GiB fixtures, 100,000–1,000,000-entry trees, and multi-hour soak tests;
- injected disk-full/short-write/fsync/rename failures at every production commit phase;
- forced cross-device moves, permission-denied trees, Windows junction swaps, and Unix non-UTF-8
  path matrices;
- process-tree tests with hanging credential helpers/hooks and operating-system job controls;
- peak RSS and open-handle reports under controlled hardware;
- database/subscriber slowdown and WebSocket overflow with production-scale event volumes.

Run these only in disposable workspaces. Capture the complete command and failure phase. A
skipped privileged case must print its capability probe and reason; it must not silently pass.
