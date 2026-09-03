# Plugin and ChatGPT setup

This guide explains how to create a ChatCMD MCP access profile, connect a local client, expose it through an operator-managed public address, connect it to ChatGPT's plugin interface, and install the optional ChatGPT browser extension.

The ChatGPT interface and terminology can change. The steps below reflect the current source and the referenced ChatCMD v1.0 walkthroughs:

- [Create a Plugin connection](https://chatcmd.net/docs/v1.0/vi/tao-agent-dau-tien)
- [Connect to ChatGPT](https://chatcmd.net/docs/v1.0/vi/ket-noi-voi-chatgpt)
- [Install the extension](https://chatcmd.net/docs/v1.0/vi/them-extension)

## 1. Start ChatCMD

Build and run the application, then open <http://127.0.0.1:8080>. See the root [README](../README.md) for source and release builds.

Keep the management page available while testing. With the default settings, a new conversation or sensitive operation may wait for your approval.

## 2. Create an access profile

1. Open **Plugin list**.
2. Select **Create new Plugin connection**.
3. Enter a short, recognizable name. Using the same name in ChatCMD and the AI client makes `@plugin-name` selection easier to understand.
4. Choose a preset or individual tools. Grant only what the use case needs. If you temporarily select all tools for diagnosis, reduce the allowlist afterward.
5. Leave **Allow connections** enabled and save.

Use separate profiles for unrelated clients or trust levels. Disabling a profile blocks its normal MCP access without deleting its history or configuration.

## 3. Connect a local MCP client

Open the profile's `•••` menu and select **Create new access code**. Copy the endpoint while it is visible:

```text
http://127.0.0.1:8080/mcp/<token>
```

Configure it as a Streamable HTTP MCP server. The URL itself is the credential; do not add it to source control, screenshots, public logs, issue reports, or shared shell history. No `Authorization` header is required.

## 4. Prepare a public address for a web-hosted AI

A web-hosted AI cannot normally reach `127.0.0.1` on your computer. Create an HTTPS reverse proxy or tunnel that you control and point it to:

```text
http://127.0.0.1:8080
```

Suitable approaches include a named Cloudflare Tunnel, another authenticated tunnel provider, a private network gateway, or a carefully firewalled reverse proxy. The open-source client does **not** include a managed ChatCMD tunnel service.

Security requirements:

- prefer a stable HTTPS hostname;
- configure tunnel access policy and firewall rules where compatible with the AI client;
- do not expose a development machine broadly without understanding the risk;
- never put the MCP token in a query string;
- remember that the public origin reaches the same HTTP listener as the UI and health endpoints;
- rotate or disable the access profile immediately if its URL leaks.

In **Plugin list**, expand **Custom Tunnel / private domain**, select **Add new domain / Tunnel**, enter only the origin (for example `https://mcp.example.com`), and save. ChatCMD validates the address by requesting:

```text
https://mcp.example.com/api/ping
```

The address is saved only when it returns the expected ChatCMD response. You can test it again from the list.

## 5. Copy a public connection link

1. Select **Copy connection link** on the desired access profile.
2. Choose the verified public address.
3. Select **Test connection** if needed.
4. Select **Copy**. The full token remains hidden on screen and is placed directly on the clipboard.

Treat the copied value as a password. ChatCMD stores public plugin-link tokens in the local SQLite database in recoverable plaintext so the same link can be copied again; protect the operating-system account, disk, backups, and database file accordingly.

## 6. Add the plugin in ChatGPT

The exact labels depend on the ChatGPT plan and current UI. If your account exposes custom plugins/MCP connections:

1. Open the ChatGPT user menu and **Settings**.
2. Under **Security and sign-in**, enable **Developer mode**.
3. Return to ChatGPT, open **Plugins**, and select the add (`+`) action.
4. Use the same name as the ChatCMD access profile.
5. Paste the public tokenized URL into **Connection**.
6. Select **No authentication** because the secret is already embedded in the path.
7. Acknowledge the developer warning, create the plugin, and select **Connect**.
8. Review the plugin's permissions. Only choose an always-allow option if that matches your risk tolerance and ChatCMD's tool allowlist is appropriately narrow.
9. Reload ChatGPT so the new plugin is discovered.

Do not repeatedly create failing plugins. If ChatGPT rate-limits plugin creation, stop and retry later after fixing the endpoint.

## 7. Test a conversation

1. In a new ChatGPT conversation, type `@` and select the configured plugin. Reload the page if it does not appear.
2. On the first request, include or select a working project folder so the runtime has a clear workspace.
3. Watch the ChatCMD management UI for the new task and approve the conversation when prompted.
4. Review every requested local action. You can allow once, allow similar actions for that scope, or reject it.
5. Confirm that progress, tool activity, file changes, terminal output, and the final response appear in the task timeline.

Once the plugin is active in a conversation, ChatGPT may not require an explicit `@name` mention for every follow-up.

## 8. Install the optional ChatGPT extension

The unpacked extension improves the ChatGPT web workflow and can surface ChatCMD approvals in the ChatGPT page. It is not required for normal MCP clients.

1. Locate `chatgpt-extension/` in the source checkout or release archive.
2. Open your Chromium browser's extensions page, for example:

   ```text
   chrome://extensions/
   edge://extensions/
   brave://extensions/
   ```

3. Enable **Developer mode**.
4. Select **Load unpacked** and choose the `chatgpt-extension` directory.
5. Confirm that **ChatCMD ChatGPT Bridge** is enabled.
6. Sign in to <https://chatgpt.com> in the same browser profile.
7. Reload both ChatGPT and the local ChatCMD page.

The extension requests `tabs`, `storage`, and `scripting` permissions plus access to `chatgpt.com` and loopback ChatCMD origins. It does not request cookie permission. Read [chatgpt-extension/README.md](../chatgpt-extension/README.md) before enabling it.

After updating ChatCMD, reload the unpacked extension from the browser extensions page. If files were replaced manually and reload is insufficient, remove the extension and load the directory again.

## 9. Verify and revoke access

- Test only with a disposable repository or folder first.
- Confirm destructive tools are absent unless explicitly required.
- Confirm approval prompts appear in the expected UI.
- Inspect **Settings → Data & logs** for local and extension diagnostics.
- Disable the profile to suspend access.
- Use **Create new access code** when a local endpoint must be replaced.
- Delete a public address when it is no longer used, and separately disable or remove it at the tunnel provider.

See [TROUBLESHOOTING.md](TROUBLESHOOTING.md) when the connection, extension, or approval flow does not work.
