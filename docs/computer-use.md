# Computer Use architecture

## Current status

ChatCMD exposes two complementary Computer Use backends:

- The browser backend starts a separate headless Chrome or Edge process with a temporary profile and
  controls the page through Chrome DevTools Protocol (CDP). It never calls desktop-wide mouse or
  keyboard injection, so the user's pointer, keyboard focus, and visible applications remain
  available while the agent works.
- The Windows desktop backend targets an already open native application window. It first uses
  Windows UI Automation for semantic element actions and Windows Graphics Capture for an isolated
  window screenshot. These operations do not require moving the user's pointer or taking keyboard
  focus. Apps that cannot be operated semantically may use a separate, explicit input-takeover mode.

```text
Model
  -> ChatCMD MCP tool schema
  -> authenticated task/agent identity + allowlist + execution approval
  -> browser: ComputerControlService -> localhost-only CDP -> isolated Chrome/Edge
  -> desktop: DesktopControlService -> UI Automation + Windows Graphics Capture
                              \-> explicit takeover -> SendInput + visible stop overlay
  -> PNG screenshot as native MCP image content
  -> Model
```

This is a custom MCP computer harness. It does not call the OpenAI API itself and does not require an
OpenAI API key. Any MCP-capable model/client can use the advertised structured tools. ChatCMD's
existing `command_run`/shell tools remain the separate code-execution capability.

## Browser MCP tools

| Tool | Purpose | Policy class |
|---|---|---|
| `computer_session_start` | Start Chrome/Edge headless with a new temporary profile | Process execution |
| `computer_observe` | Return URL/title/viewport metadata plus a PNG MCP image | Content read |
| `computer_act` | Run a bounded batch of structured browser actions | Mutation |
| `computer_session_close` | Kill the browser and delete its temporary profile | Cleanup |

Normal loop:

```text
computer_session_start
  -> computer_observe
  -> computer_act (click/type/scroll/...)
  -> computer_observe or computer_act with screenshotAfter=true
  -> repeat until done
  -> computer_session_close
```

`computer_act.actions` supports `click`, `double_click`, `move`, `drag`, `scroll`, `keypress`,
`type`, `navigate`, `wait`, and `screenshot`. Coordinates are viewport pixels from the last
observation. A `screenshot` action forces an image result even when `screenshotAfter=false`.

## Windows desktop MCP tools

| Tool | Purpose | Policy class |
|---|---|---|
| `desktop_window_list` | List targetable top-level application windows and return opaque window IDs | Metadata read |
| `desktop_window_observe` | Capture one window and optionally enumerate its UI Automation elements | Content read |
| `desktop_element_act` | Invoke a semantic UI Automation action from one observation | Mutation |
| `desktop_input_begin` | Start explicit foreground input takeover and show its stop overlay | Process execution |
| `desktop_input_act` | Run bounded mouse/keyboard actions in the active takeover | Mutation |
| `desktop_input_end` | Stop takeover and remove its overlay | Cleanup |

The preferred desktop loop stays in the background:

```text
desktop_window_list
  -> choose exactly one returned windowId
  -> desktop_window_observe
  -> desktop_element_act (invoke/set_value/toggle/select/expand/collapse)
  -> desktop_window_observe
  -> repeat until done
```

An `observationId` and its `elementId` values are point-in-time capabilities. A semantic action
invalidates that observation. Window layout, modality, focus, interleaved user activity, failure, or
retry also requires a fresh observation; callers must never reuse a stale element ID.

### Explicit input takeover

Some legacy, canvas, game, or custom-rendered interfaces expose no usable UI Automation patterns.
ChatCMD does not silently fall back to physical input. After the normal authorization/approval flow,
the caller must explicitly start takeover:

```text
desktop_input_begin
  -> desktop_input_act (click/type/drag/scroll/keypress/wait)
  -> observe and verify as needed
  -> desktop_input_end
```

Takeover brings the selected target to the foreground and can therefore contend with the user for
the shared mouse and keyboard. While it is active, ChatCMD displays a click-through, always-on-top,
blinking border around the target and a banner reading **Computer control active — Press ESC to
stop**. ESC is a global emergency stop: it cancels the takeover and removes the overlay. If the user
switches focus away from the exact target window, ChatCMD fails closed and cancels before sending the next input;
it never continues typing into the newly focused app. Only one takeover may be active at a time.

`desktop_input_act` coordinates are relative to the selected window and are bounds checked. Windows
key/Meta shortcuts are rejected. A stopped, cancelled, expired, or foreign task's input session is
not reusable.

## Isolation and safety properties

- Browser UI is headless; CDP actions do not use Windows `SendInput`.
- Each session has a random ID and is bound to the authenticated agent plus task.
- Each session gets a fresh temporary browser profile; user cookies/history are not reused.
- DevTools binds to `127.0.0.1` with an ephemeral port.
- Browser downloads are denied through CDP.
- Explicit navigation accepts only `http`, `https`, and `about:blank`; `file:`, `javascript:`, and
  browser-internal schemes are rejected.
- At most four live sessions and 32 actions per batch are accepted.
- Viewports, coordinates, waits, drag duration, typed text, key chords, scroll deltas, and screenshot
  size are bounded before execution.
- Actions within one session are serialized.
- Screenshot bytes are removed from structured/timeline persistence. Timeline events keep only a
  bounded redacted summary; the MCP response promotes the PNG to native image content.
- Start, observe, and act use the existing ChatCMD execution policy/approval flow. Tool allowlists
  still apply; an existing agent must be granted the new Computer Use tools in local settings.
- Desktop window IDs, observation IDs, element IDs, and input-session IDs are opaque and bound to the
  authenticated agent plus task. Native window handles are never accepted from the model.
- Desktop screenshots are scoped to the selected window and can work while another window occludes
  it. A minimized window may not produce a current frame and fails clearly instead of being restored
  behind the user's back.
- Semantic desktop actions use only patterns advertised by the selected UI Automation element.
  Password fields cannot be read or filled through this backend.
- The desktop target filter excludes terminals and the Windows Run dialog, authentication and
  credential surfaces, password managers, Windows security/anti-malware apps, and ChatGPT/Codex/
  ChatCMD itself. These are hard denies, not approval prompts.
- Content displayed by an app is untrusted input. Sending messages, submitting forms, uploading,
  deleting, sharing data, or other externally visible side effects still require the applicable
  execution approval immediately before the action.

These controls protect desktop input and browser profile state, but they are not an OS security
boundary. Both backends run as the ChatCMD user. UI Automation cannot operate elevated windows from
a lower-integrity process, and takeover necessarily shares the interactive desktop. For a strict
non-interference guarantee across legacy apps, games, elevated windows, and custom-rendered canvases,
run automation inside a separate Windows user session or VM; a Windows virtual desktop alone is not
a full input or security boundary.

## Verification

The implementation includes contract, authorization, redaction, validation, and catalog tests.
Browser smoke tests are ignored by default because they require an installed browser:

```powershell
cargo test -p chatcmd-runtime --lib isolated_chrome_can_capture_without_desktop_input -- --ignored
cargo test -p chatcmd-runtime --lib isolated_chrome_executes_pointer_free_actions -- --ignored
cargo test -p chatcmd-runtime --test desktop_contract desktop_window_binding_smoke_does_not_require_foreground_input -- --ignored --exact
```

The second browser test serves a local page, starts isolated Chrome, clicks an input, types text,
verifies the screenshot changed, and closes the session. The desktop smoke test only enumerates and
binds to an eligible window with screenshots and UI elements disabled; it never starts takeover or
injects input.
