# Troubleshooting

Start with the smallest reproducible configuration: loopback binding, default port, one access profile, a narrow tool allowlist, and no public tunnel.

## The management page does not open

1. Confirm the Rust process is still running.
2. Open <http://127.0.0.1:8080/api/health> and <http://127.0.0.1:8080/api/info>.
3. Check whether another process owns port `8080`.
4. If `CHATCMD_PORT` is set, open that port instead.
5. For a non-embedded build, build `web/dist` or use the Vite development server.

```bash
cd web
npm ci
npm run build
cd ..
cargo run
```

If the API works but frontend assets do not, verify `web/dist/index.html` or set `CHATCMD_WEB_DIST` to the correct build directory.

## Port 8080 is already in use

Set another valid port before starting:

```powershell
$env:CHATCMD_PORT = "8081"
cargo run
```

Update the extension/local client configuration and any reverse proxy to match. Vite's checked-in proxy targets port `8080`; adjust `web/vite.config.ts` for a different backend during frontend development.

## The MCP client reports unauthorized

- Verify the entire `/mcp/<token>` URL was copied with no spaces or punctuation.
- Do not move the token into `?token=`, `?access_token=`, or `?bearer_token=`; query credentials are rejected.
- Confirm the access profile is enabled.
- If a new access code was created, replace the old local endpoint in the client.
- Create a fresh access code if the URL may have been truncated or exposed.

## The MCP client reports an origin or host error

Browser origins are limited to the configured local `localhost`/`127.0.0.1` origin. Native clients may omit `Origin` only while ChatCMD is bound to loopback. Binding directly to a non-loopback address enables stricter host validation and denies origin-less clients.

Prefer leaving `CHATCMD_BIND=127.0.0.1` and using a well-configured tunnel/reverse proxy on the same machine. Inspect proxy host/header behavior before weakening a boundary.

## A public address cannot be saved or tested

ChatCMD requests `<origin>/api/ping` and expects a successful JSON response with `pong: true` and `service: ChatCMD`.

- Confirm the tunnel targets the current ChatCMD port.
- Enter only the origin—no MCP path, token, query, fragment, username, or password.
- Verify DNS and TLS from the ChatCMD machine.
- Check tunnel access rules; an interactive login page will fail the probe.
- Check that the tunnel provider did not rewrite `/api/ping`.
- Do not add `localhost` or `127.0.0.1` as a public address.

## ChatGPT cannot create or discover the plugin

- Confirm Developer mode is available and enabled for the current account/workspace.
- Use the verified public tokenized URL, not `localhost`.
- Select **No authentication**; the URL path is already the bearer credential.
- Reload ChatGPT after creating the plugin.
- Type `@` and search for the exact configured name.
- If plugin creation was attempted repeatedly, wait before trying again.
- Check for account, workspace, plan, or administrator restrictions in ChatGPT.

## The browser extension is not detected

1. Open the browser's extensions page.
2. Confirm Developer mode and **ChatCMD ChatGPT Bridge** are enabled.
3. Reload the unpacked extension after every source update.
4. Reload both `chatgpt.com` and the local ChatCMD page.
5. Use the same browser profile for the extension and the signed-in ChatGPT tab.
6. Open **Settings → Data & logs → Extension logs**.

The bridge accepts local callbacks only from HTTP `localhost` or `127.0.0.1`. It will not use a remote ChatCMD management origin.

## ChatGPT sending, stopping, or response capture broke

The extension is DOM-based and third-party UI changes can invalidate selectors. Run:

```bash
cd chatgpt-extension
node --test content-chatgpt.test.cjs
```

Then inspect the browser extension service worker console and extension logs. Record the ChatGPT UI variant and visible model label in a bug report, but redact conversations and credentials.

## A new conversation is waiting

The default configuration requires approval for new ChatGPT conversations. Open the ChatCMD page and approve or reject the waiting conversation. With the extension enabled, the same approval may appear inside ChatGPT.

If approvals are intentionally unnecessary on a trusted single-user machine, review **Settings → Execution → Approve new conversations**. Disabling it broadens automatic access.

## A tool is waiting for approval

Approval mode pauses sensitive operations. Review the exact tool and input, then choose:

- allow this request;
- allow similar requests in the supported scope;
- reject.

If no dialog is visible, reload the management page and check pending approvals, the task timeline, the encrypted WebSocket connection, and application logs.

## A terminal is busy or input is locked

Interactive input is disabled while an agent owns the terminal. Wait for the active operation, stop that activity from the task timeline, or close the session if interruption is acceptable. After an unexpected restart, stale sessions are marked interrupted and cannot be resumed as live processes.

## Completion is shown as not run, unknown, stale, or failed

- `notRun`: no execution evidence was supplied. Legacy clients that only send final content use this safe default.
- `unknown`: an execution ID is missing/wrong-owner/lost after restart, criterion coverage is incomplete, or source freshness was not captured.
- `stale`: evidence belongs to an earlier turn or captured source state changed.
- `failed`: the referenced command exited non-zero, timed out, or was cancelled.
- `notApplicable`: valid only for review/docs-only work with an explicit reason.

An exit-0 `command_run` may currently remain `unknown` because source fingerprints are not yet
captured. Do not work around this by claiming a boolean pass or parsing `PASS` from terminal text.
Re-run only when the operation is safe and a prior execution is known terminal; after connection
loss, reuse the same idempotency key so the runtime can observe the original in-flight execution.

## The UI says “Request failed (200)”

An HTTP handler may have succeeded while the browser failed to decrypt the response. Check:

- `X-ChatCmd-Crypto: 1` on the response;
- whether the Rust backend restarted and the browser performed one session reset/retry;
- method, full `/api/local/...` path, query ordering, and status used as AES-GCM associated data;
- proxy or middleware changes that modified the request URI/body;
- browser console errors.

See [ENCRYPTION_PROTOCOL.md](ENCRYPTION_PROTOCOL.md) for the full checklist.

## The database cannot open or migrate

- Verify the parent directory is writable.
- Check free disk space and filesystem permissions.
- Confirm `CHATCMD_DB_PATH` points to a file, not a directory.
- Do not open the same database with incompatible application builds.
- Back up the database before manual inspection or repair.

ChatCMD rejects a database with a schema newer than the current binary rather than attempting a downgrade.

## Where to find diagnostics

- Dashboard: runtime, MCP, database, task, terminal, and approval status.
- **Settings → Data & logs**: SQLite metrics, application logs, extension logs, retention, and cleanup.
- Default application log: `logs/chatcmd.log`.
- Override log path: `CHATCMD_LOG_PATH`.
- Rust tracing: configure `RUST_LOG` before launch.

Review logs before sharing them. Remove endpoint tokens, private paths, conversations, user data, and proprietary source content.

If the problem remains, follow [SUPPORT.md](../SUPPORT.md) and use the appropriate issue template.
