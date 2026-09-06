# Adversarial coding-agent tests

This suite separates deterministic contract evidence from model evaluation. It never calls a
network service or live model.

## Reproduction

Run the manifest and simulated-state harness:

```text
cargo test -p chatcmd-mcp --test coding_behavior_harness
node --test crates/chatcmd-mcp/tests/coding_fixtures/regression/regression.test.mjs
```

The case manifest is `crates/chatcmd-mcp/tests/coding_fixtures/cases.json`. It records a fixed
seed, disabled-network policy, supported toolchain assumptions, fixture hashes, request text,
allowed effects/files, initial Git state, hidden invariants, and the existing test that supplies
behavioral evidence. The artifact contract in `artifact-schema.json` requires instructions hash,
redacted transcript, before/after diff, execution evidence, rubric result, host/model/config, and
date. Fake secret text uses a valueless sentinel.

## Coverage matrix

| Case | Tier | Deterministic or simulated evidence | Status |
|---|---:|---|---|
| E01 | A | Harness constrains review to read-only and no changed files | covered |
| E02 | A | Harness permits only the requested plan artifact | covered |
| E03 | A | Regression fixture requires fail-before/pass-after with unchanged assertion | covered |
| E04 | A | Cross-layer fixture enumerates schema/runtime/API/UI and acceptance | covered |
| E05 | A | Git scope fixture preserves unrelated staged content | covered |
| E06 | A | Git fixture rejects a selected file containing mixed user hunks | covered |
| E07 | A | Blocked/not-run cannot become verified | covered |
| E08 | A | Baseline fixture computes and reports the new-failure set separately | covered |
| E09 | A | Consent timeout/reject/custom tests fail closed | covered |
| E10 | A | Deny-mode authorization test proves no process spawn | covered |
| E11 | B | Packaged initialize/list contract plus discovery-before-call simulation | covered |
| E12 | A | Truncation continuation test plus reread simulation | covered |
| E13 | A | Atomic write rejects stale version without overwrite | covered |
| E14 | A | Injection fixture remains data; sentinel is absent from transcript | covered |
| E15 | A | Root/nested rule scope and sibling isolation test | covered |
| E16 | B | Shared sampling prompt and real tool-schema test | covered |
| E17 | B | Fake no-sampling client exercises extension fallback without failure | covered |
| E19 | A | Command result keeps printed `PASS` separate from non-zero exit | covered |
| E20 | A | Command-runner evidence distinguishes cancellation and bounds output flood | covered |
| E21 | B | Cached catalog refreshes once; additive defaults do not grant permission | covered |
| E22 | A | Blocked/partial/failed outcomes are terminal and unverified | covered |
| E23 | A | Quoted/code/negated planning text grants no authority | covered |

Tier B evidence uses the existing packaged-process smoke and fake sampling clients. It is a local
simulated-host result, not live AI validation. The harness intentionally reuses runtime and release
tests rather than duplicating their implementations.

## Gaps and live status

The deterministic/simulated denominator is 22 of 24 matrix cases, with 22 covered and 0 failed in
the focused harness. This number measures contract/fixture coverage, not live-model coding quality.
E18 remains unclaimed pending lifecycle evidence that a parent re-verifies after integration changes.
E24 is **MANUAL UI / NOT RUN**: no browser workflow was invoked, so build or schema evidence is not
presented as keyboard, focus, error-state, or timeout validation.

Tier C is **BLOCKED / NOT RUN** as of 2026-09-05. No live host/model, quota, or isolated live-run
configuration was supplied, and this change deliberately made no network calls. To run Tier C,
provide an isolated disposable fixture runner and record host/model/version/config/date, a redacted
tool transcript, source diff, test output, instructions hash, and at least three same-seed attempts
per selected case. Until then, do not label this coding profile “live validated.”

The adversarial workflow is evidence-only: pull-request, scheduled, and manual runs execute tests
and upload benchmark artifacts, but never publish a desktop release. Desktop DEV packaging remains
manual-only through `workflow_dispatch` in `build-desktop-dev-release.yml`.
