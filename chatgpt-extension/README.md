# ChatCMD ChatGPT Bridge

ChatCMD ChatGPT Bridge is an optional Chromium Manifest V3 extension that connects the local ChatCMD console to an already signed-in `chatgpt.com` tab.

It is an unofficial browser UI integration, not the OpenAI API and not a replacement for a normal MCP connection. It automates the current ChatGPT page DOM in the same browser profile as the user.

## Capabilities

- Start a ChatGPT conversation from the local ChatCMD UI.
- Continue an existing conversation while retaining its browser identity.
- Select a model by its visible ChatGPT label.
- Queue, reorder, edit, send immediately, or delete follow-up messages.
- Stop an active response.
- Relay assistant output and conversation URLs to the matching local task.
- Surface ChatCMD conversation, tool, and plan-question approvals in ChatGPT.
- Provide a browser fallback for sub-agent work when host sampling is unavailable.
- Keep bounded diagnostic logs in extension storage for local troubleshooting.

## Install for development

1. Start ChatCMD on `http://127.0.0.1:8080` or `http://localhost:8080`.
2. Open `chrome://extensions/`, `edge://extensions/`, or `brave://extensions/`.
3. Enable **Developer mode**.
4. Select **Load unpacked**.
5. Choose this `chatgpt-extension` directory.
6. Sign in to <https://chatgpt.com> in the same browser profile.
7. Reload the ChatGPT and local ChatCMD pages.

For the complete MCP profile, public address, ChatGPT plugin, and extension workflow, read [Plugin and ChatGPT setup](../docs/PLUGIN_SETUP.md).

## Permissions and trust model

The manifest requests:

- `tabs` to find, open, focus, and close ChatGPT conversation tabs;
- `storage` for conversation bindings, request context, preferences, and bounded diagnostics;
- `scripting` to restore content scripts after extension or page lifecycle changes;
- host access to `https://chatgpt.com/*` and local HTTP `localhost`/`127.0.0.1` origins.

The extension does **not** request the `cookies` permission and does not read or write ChatGPT login tokens. It can still read and interact with the visible ChatGPT page because that is its purpose. Use a dedicated browser profile if stronger separation is required.

Callbacks are restricted to local HTTP origins and include:

```text
X-ChatCmdClient: chatgpt-extension
```

The approval WebSocket uses the same ephemeral encrypted session model as the local UI. This does not protect data from a compromised browser profile, malicious extension, injected page code, or local administrator.

## Tab behavior

- An existing ChatGPT conversation tab is reused when possible.
- If ChatCMD creates a background tab for a request, the extension can close that generated tab after completion.
- A tab that the user already had open is not automatically closed.
- Conversation IDs and provisional-to-final URL aliases are stored so follow-up messages return to the correct tab.

## Model selection

`Auto` keeps the current/default ChatGPT model. For another value, the extension opens the model switcher and selects the visible label. Available names depend on the account, workspace, plan, and current ChatGPT UI, so ChatCMD accepts a custom label.

## Diagnostics

Open **ChatCMD → Settings → Data & logs → Extension logs**. For background failures, also inspect the extension service worker from the browser extensions page.

Logs and bug reports must not contain private conversations, MCP endpoints, cookies, credentials, or proprietary source data.

## Test

```bash
node --test content-chatgpt.test.cjs
```

The automated suite covers DOM-adapter behavior with fixtures. After changing selectors or message flow, manually verify start, continue, model selection, stop, approvals, queue handling, tab reuse, and final-response capture in a disposable browser profile.

## Maintenance limitation

ChatGPT's page structure, labels, and interaction behavior can change without notice. Selectors are split across `content-chatgpt-ui.js`, `content-chatgpt-dom.js`, `content-chatgpt-approval-ui.js`, and `content-chatgpt.js` to keep updates localized, but every significant ChatGPT UI change should trigger a manual compatibility test.

## License

This extension is part of ChatCMD and is distributed under the repository's [MIT License](../LICENSE).
