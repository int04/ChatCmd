// Short, bounded capture wakeups owned by the service worker, not by the hidden page.
// This endpoint sends no conversation content and grants no filesystem/tool authority.
(() => {
  const pending = new Map();
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message?.type !== 'chatcmd-capture-wake') return false;
    const tabId = sender.tab?.id;
    if (!tabId || (sender.frameId !== undefined && sender.frameId !== 0)
      || !isChatGptUrl(sender.url || sender.tab?.url)) {
      sendResponse({ ok: false, error: 'Capture clock requires the top-level ChatGPT content script.' });
      return false;
    }
    const jobs = pending.get(tabId) || new Set();
    if (jobs.size >= 4) {
      sendResponse({ ok: false, error: 'Too many pending capture wakeups.' });
      return false;
    }
    const delayMs = Math.min(1000, Math.max(25, Number(message.delayMs) || 25));
    let timer;
    const finish = () => {
      clearTimeout(timer);
      jobs.delete(finish);
      if (!jobs.size) pending.delete(tabId);
      try { sendResponse({ ok: true, clockProtocol: 1 }); } catch { /* document was destroyed */ }
    };
    jobs.add(finish);
    pending.set(tabId, jobs);
    timer = setTimeout(finish, delayMs);
    return true;
  });
  chrome.tabs.onRemoved.addListener((tabId) => {
    for (const finish of [...(pending.get(tabId) || [])]) finish();
  });
})();
