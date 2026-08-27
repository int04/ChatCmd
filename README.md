# ChatCmdClient

ChatCmdClient is a local-only bridge between AI clients, MCP, a bounded machine runtime, SQLite, and a React/Vite management UI. It has no SaaS account, quota, payment, or remote authentication dependency.

## Run

Requirements: stable Rust, Node.js/npm, Git, and a supported local shell.

```powershell
cd web
npm ci
npm run build
cd ..
cargo run
```

Open `http://127.0.0.1:8080`. The listener defaults to `127.0.0.1:8080`. Override it with `CHATCMD_BIND` and `CHATCMD_PORT`; exposing a non-loopback address disables origin-less MCP clients. Override the database with `CHATCMD_DB_PATH`.

Default SQLite paths:

- Windows: `%LOCALAPPDATA%\ChatCmdClient\data\chatcmd.db`
- macOS: `~/Library/Application Support/ChatCmdClient/chatcmd.db`
- Linux: `$XDG_DATA_HOME/chatcmd-client/chatcmd.db`, or `~/.local/share/chatcmd-client/chatcmd.db`

Startup is idempotent. Stale running tasks and terminal sessions become `interrupted` after restart.

## MCP

Endpoint format: `http://127.0.0.1:8080/mcp/{token}`

Create an agent in the UI, select its allowed tools, then save the returned MCP URL immediately. The generated token is embedded as the final path segment. No `Authorization` header is used or required. The complete tokenized URL appears only after create or rotate and is never returned by list/get APIs.

```text
http://127.0.0.1:8080/mcp/<one-time-token>
```

Browser origins are restricted to `localhost` or `127.0.0.1` on the configured port. Native MCP clients may omit `Origin` only while the server is bound to a loopback address. Query-string tokens (`token`, `access_token`, `bearer_token`) are rejected. The built-in HTTP trace masks `/mcp/{token}`, but client history and external proxy logs may still expose a URL token, so keep the endpoint local and private.

Management API requests require `X-ChatCmdClient: local-ui`. Static files, WebSocket, health/info, and MCP do not require this UI marker. No permissive CORS layer is enabled.

## Verify

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd web
npm ci
npm run lint
npm test -- --run
npm run build
```
