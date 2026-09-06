(() => {
  const slots = new Map();
  function install(kind) {
    const token = crypto.randomUUID();
    const attribute = `data-chatcmd-${kind}-context`;
    document.documentElement.setAttribute(attribute, token);
    const context = Object.freeze({ kind, token, attribute });
    slots.set(kind, context);
    return context;
  }
  function current(context) {
    return Boolean(context) && document.documentElement.getAttribute(context.attribute) === context.token;
  }
  function invalidated(error) {
    return /extension context invalidated/i.test(String(error?.message || error || ''));
  }
  function sendMessage(message, callback) {
    try {
      if (!chrome.runtime?.id) throw new Error('Extension context invalidated.');
      if (callback) {
        chrome.runtime.sendMessage(message, (response) => {
          let error;
          try { error = chrome.runtime.lastError?.message; }
          catch { error = 'Extension context invalidated.'; }
          callback(response, error ? new Error(error) : undefined);
        });
        return undefined;
      }
      return chrome.runtime.sendMessage(message);
    } catch (error) {
      if (!callback) return Promise.reject(error);
      queueMicrotask(() => callback(undefined, error));
      return undefined;
    }
  }
  globalThis.ChatCmdRuntime = Object.freeze({ current, install, invalidated, sendMessage });
})();
