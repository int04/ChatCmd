const REQUEST_TYPE = 'chatcmd-chatgpt-extension-request';
const RESPONSE_TYPE = 'chatcmd-chatgpt-extension-response';
const CONTENT_CONTEXT = globalThis.ChatCmdRuntime.install('chatcmd');

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== 'chatcmd-content-alive' || message.kind !== 'chatcmd') return false;
  sendResponse({ ok: true, kind: 'chatcmd' });
  return false;
});

window.addEventListener('message', (event) => {
  if (!globalThis.ChatCmdRuntime.current(CONTENT_CONTEXT)) return;
  if (event.source !== window || event.origin !== window.location.origin) return;
  const message = event.data;
  if (!message || message.type !== REQUEST_TYPE || typeof message.nonce !== 'string') return;
  const payload = { ...message, type: 'chatcmd-local-command' };
  globalThis.ChatCmdRuntime.sendMessage(payload, (response, runtimeError) => {
    if (!globalThis.ChatCmdRuntime.current(CONTENT_CONTEXT)) return;
    if (runtimeError && globalThis.ChatCmdRuntime.invalidated(runtimeError)) return;
    window.postMessage({
      type: RESPONSE_TYPE,
      nonce: message.nonce,
      ...(response && typeof response === 'object' ? response : {}),
      ok: Boolean(response?.ok) && !runtimeError,
      error: runtimeError?.message || response?.error,
    }, window.location.origin);
  });
});
