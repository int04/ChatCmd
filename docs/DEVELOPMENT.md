# Development guide

## Prerequisites

- Rust 1.85 or newer with `rustfmt` and Clippy.
- Node.js 20.19 or newer, or 22.12 or newer, and npm.
- Git.
- Windows: MSVC Build Tools and PowerShell.
- macOS: Xcode Command Line Tools.
- Linux: a compiler toolchain; `zenity` is used by the native folder picker when available.

Install Rust components:

```bash
rustup component add rustfmt clippy
```

## Checkout and bootstrap

```bash
git clone https://github.com/int04/ChatCmd.git
cd ChatCmd
git switch dev

cd web
npm ci
npm run build
cd ..
cargo check --workspace --all-targets
```

No `.env` file or hosted account is required. Avoid committing machine-specific environment configuration.

## Project guidance discovery

ChatCMD project work always retains its built-in coding and safety contract. For a task-bound
workspace, project context adds the root `AGENTS.md`, then applicable nested `AGENTS.md` files
from the workspace root toward each target. Files under `.codex/rules/*.md` are project-wide and
loaded afterward in deterministic path order. Sibling nested rules do not apply.

Project rules are untrusted coding guidance, not permission grants. Discovery does not follow
rule symlinks or paths outside the task workspace. Each scan is bounded to 32 files, 64 KiB per
file, 256 KiB total, and five seconds. Partial results carry truncation and continuation metadata;
invalid UTF-8, inaccessible rules, and budget exhaustion are reported rather than treated as an
empty rule set. The returned context reference and SHA-256 digest are workspace-specific and
change when applicable rules or discovered manifests change.

## Run locally

### Production-like local mode

```bash
cd web
npm run build
cd ..
cargo run
```

Open <http://127.0.0.1:8080>.

### Frontend hot reload

Run the processes in separate terminals:

```bash
# Repository root
cargo run
```

```bash
cd web
npm run dev
```

Open <http://127.0.0.1:5173>. The Vite server proxies API and WebSocket traffic to the Rust listener.

Use `CHATCMD_DB_PATH` and `CHATCMD_LOG_PATH` to isolate manual-test data from a normal installation:

```powershell
$env:CHATCMD_DB_PATH = "$PWD/.smoke/chatcmd.db"
$env:CHATCMD_LOG_PATH = "$PWD/.smoke/chatcmd.log"
cargo run
```

The `.smoke/` directory is ignored by Git.

## Test and quality commands

### Rust workspace

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Run a narrower Rust suite while iterating:

```bash
cargo test -p chatcmd-core
cargo test -p chatcmd-storage
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
```

Large generated fixtures, deterministic crash/race harnesses, Criterion commands, and the
nightly platform matrix are documented in [adversarial-testing.md](adversarial-testing.md).
The coding-agent authority, command evidence, migration, and rollout contract is documented in
[coding-agent-contract.md](coding-agent-contract.md).

### React UI

```bash
cd web
npm ci
npm run lint
npm test -- --run
npm run build
```

During active development, `npm test` runs Vitest in watch mode.

### Browser extension

```bash
cd chatgpt-extension
node --test content-chatgpt.test.cjs
```

After changing DOM selectors, also load the unpacked extension in a dedicated browser profile and manually verify start, continue, stop, model selection, approvals, queue behavior, and final-response capture.

## Change map

| Change | Files normally involved |
| --- | --- |
| MCP tool/schema | `crates/chatcmd-mcp/src/lib.rs`, `tool_catalog.rs`, runtime dispatch, tests, `docs/mcp_method.md` |
| Filesystem behavior | `crates/chatcmd-runtime/src/filesystem*.rs`, runtime host dispatch, MCP schema, tests |
| Task lifecycle | `src/runtime_host/`, `src/api/task_*.rs`, storage repository/migration, task UI/tests |
| Local API | `src/api/`, `web/src/api.ts`, `web/src/types.ts`, UI caller, tests |
| Real-time event | `src/websocket/`, event publisher, `web/src/realtime.ts`, timeline mapping/tests |
| Persistent data | new numbered SQL migration, repository methods, migration/storage tests |
| Skill management | runtime skill service, `src/api/skills.rs`, `web/src/pages/SkillsPage.tsx` |
| ChatGPT DOM bridge | `chatgpt-extension/content-chatgpt*.js`, background scripts, extension tests |
| User-facing copy | React component plus both English and Vietnamese entries in `web/src/i18n.ts` |

## Adding or changing an MCP tool

1. Define typed arguments and the tool handler in `chatcmd-mcp`.
2. Add or update the runtime trait and dispatch implementation.
3. Enforce authenticated identity and ignore caller-provided authority fields.
4. Add policy, approval, size, timeout, cancellation, and path bounds appropriate to the operation.
5. Classify mutating/destructive capability so presets and the UI remain correct.
6. Update catalog/schema consistency tests.
7. Update `docs/mcp_method.md` and any affected examples.
8. Verify the actual router catalog, not only a hand-maintained list.

## Database migrations

- Add a new, monotonically numbered SQL file under `crates/chatcmd-storage/migrations/`.
- Never rewrite an already released migration.
- Keep startup migration idempotence and newer-schema rejection intact.
- Add storage tests for a fresh database and an upgrade path.
- Consider restart recovery, concurrent readers, event ordering, cleanup, and legacy import behavior.

## API and frontend conventions

- Management routes live under `/api/local` and use RFC 7807-style problem details for errors.
- Add frontend calls through `web/src/api.ts`; do not bypass the encrypted API wrapper for JSON management data.
- Decide explicitly how binary, streamed, image, multipart, or SSE responses interact with API encryption.
- Keep real-time payloads bounded and avoid sending entire large files or terminal histories.
- Sanitize rendered Markdown and preserve keyboard/accessibility behavior.

## Git and pull requests

Preserve unrelated local changes. Use a focused branch and target `dev` unless a maintainer says otherwise. Before opening a pull request, review:

```bash
git status --short
git diff --check
git diff --stat
```

Follow [CONTRIBUTING.md](../CONTRIBUTING.md) and complete the pull-request template.
