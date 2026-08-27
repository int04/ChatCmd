const REQUEST_TYPE = 'chatcmd-chatgpt-extension-request';
const RESPONSE_TYPE = 'chatcmd-chatgpt-extension-response';

window.addEventListener('message', (event) => {
  if (event.source !== window || event.origin !== window.location.origin) return;
  const message = event.data;
  if (!message || message.type !== REQUEST_TYPE || typeof message.nonce !== 'string') return;
  const payload = { ...message, type: 'chatcmd-local-command' };
  chrome.runtime.sendMessage(payload, (response) => {
    const error = chrome.runtime.lastError?.message;
    window.postMessage({
      type: RESPONSE_TYPE,
      nonce: message.nonce,
      ...(response && typeof response === 'object' ? response : {}),
      ok: Boolean(response?.ok) && !error,
      error: error || response?.error,
    }, window.location.origin);
  });
});
