const REQUEST_PREFIX = 'chatcmd-request:';
const CONVERSATION_PREFIX = 'chatcmd-conversation:';
const CHATGPT_HOME = 'https://chatgpt.com/';

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message || typeof message !== 'object') return false;
  if (message.type === 'chatcmd-local-command') {
    if (message.action === 'ping') {
      void chatGptTabStatus(message.conversationUrl)
        .then((status) => sendResponse({ ok: true, ...status }))
        .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'send') {
      try {
        const localBaseUrl = localOrigin(message.localBaseUrl);
        void startRequest({ ...message, localBaseUrl }).catch((error) => void reportFailure(message.requestId, localBaseUrl, error));
        sendResponse({ ok: true });
      } catch (error) { sendResponse({ ok: false, error: errorMessage(error) }); }
      return false;
    }
    if (message.action === 'stop') {
      void stopRequest(message).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
  }
  if (message.type === 'chatcmd-chatgpt-progress') {
    void handleProgress(message, sender.tab?.id).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  return false;
});

chrome.tabs.onRemoved.addListener((tabId) => {
  void handleClosedTab(tabId);
});

async function startRequest(message) {
  if (!message.requestId || !message.submittedContent) throw new Error('Yêu cầu gửi ChatGPT không hợp lệ.');
  const target = conversationTarget(message.conversationUrl);
  const tab = message.conversationUrl
    ? await acquireConversationTab(target)
    : await acquireNewConversationTab();
  await chrome.storage.session.set({
    [requestKey(message.requestId)]: {
      localBaseUrl: message.localBaseUrl,
      tabId: tab.id,
      conversationUrl: message.conversationUrl || null,
    },
  });
  await sendToChatGpt(tab.id, {
    type: 'chatcmd-chatgpt-run',
    requestId: message.requestId,
    submittedContent: message.submittedContent,
    model: message.model || 'Auto',
  });
}

async function stopRequest(message) {
  localOrigin(message.localBaseUrl);
  if (!message.requestId) throw new Error('Thiếu request ID cần dừng.');
  const context = await requestContext(message.requestId);
  if (!context?.tabId) throw new Error('Không tìm thấy tab ChatGPT đang xử lý yêu cầu này.');
  await chrome.tabs.sendMessage(context.tabId, { type: 'chatcmd-chatgpt-stop', requestId: message.requestId });
}

async function handleProgress(message, tabId) {
  if (!message.requestId) throw new Error('ChatGPT progress thiếu request ID.');
  const context = await requestContext(message.requestId);
  if (!context) throw new Error('Không tìm thấy ChatCMD request context.');
  if (tabId && context.tabId !== tabId) throw new Error('ChatGPT progress đến từ tab không khớp.');
  if (message.stage === 'started') {
    if (message.conversationId && context.tabId) await bindConversationTab(message.conversationId, context.tabId);
    await postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/started`, {
      conversationId: message.conversationId,
      conversationUrl: message.conversationUrl,
      model: message.model,
      userText: message.userText,
    });
    return;
  }
  if (message.stage === 'result') {
    await postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/result`, {
      status: message.status,
      conversationId: message.conversationId,
      conversationUrl: message.conversationUrl,
      assistantContent: message.assistantContent,
      errorMessage: message.errorMessage,
    });
    await releaseRequest(message.requestId);
  }
}

async function reportFailure(requestId, localBaseUrl, error) {
  if (!requestId) return;
  try {
    await postJson(localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(requestId)}/result`, {
      status: 'failed',
      errorMessage: errorMessage(error),
    });
  } catch { /* the local app may already be closed */ }
  await releaseRequest(requestId);
}

async function handleClosedTab(tabId) {
  const stored = await chrome.storage.session.get(null);
  const removals = [];
  const failures = [];
  for (const [key, value] of Object.entries(stored)) {
    if (!value || typeof value !== 'object' || value.tabId !== tabId) continue;
    if (key.startsWith(CONVERSATION_PREFIX)) removals.push(key);
    if (key.startsWith(REQUEST_PREFIX) && value.localBaseUrl) {
      const requestId = key.slice(REQUEST_PREFIX.length);
      failures.push(reportFailure(requestId, value.localBaseUrl, new Error('Tab ChatGPT liên kết với cuộc trò chuyện đã bị đóng. Mở lại cuộc trò chuyện ChatGPT để tiếp tục.')));
    }
  }
  if (removals.length) await chrome.storage.session.remove(removals);
  await Promise.allSettled(failures);
}

async function chatGptTabStatus(conversationUrl) {
  if (!conversationUrl) {
    const tab = await findAvailableNewConversationTab();
    return { chatGptTabOpen: Boolean(tab?.id), tabId: tab?.id, tabUrl: tab?.url };
  }
  const target = conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  return { chatGptTabOpen: Boolean(tab?.id), conversationTabOpen: Boolean(tab?.id), tabId: tab?.id, tabUrl: tab?.url };
}

async function acquireNewConversationTab() {
  const tab = await findAvailableNewConversationTab();
  if (!tab?.id) throw new Error('Không có tab ChatGPT trống để tạo cuộc trò chuyện mới. Hãy mở một tab https://chatgpt.com/ mới rồi thử lại.');
  await waitForTab(tab.id);
  return tab;
}

async function acquireConversationTab(target) {
  const tab = await findConversationTab(target);
  if (!tab?.id) throw new Error('Tab ChatGPT của cuộc trò chuyện này không còn mở. Hãy mở lại link cuộc trò chuyện rồi thử lại.');
  await waitForTab(tab.id);
  return tab;
}

async function findAvailableNewConversationTab() {
  const tabs = await chatGptTabs();
  const bindings = await conversationBindings();
  const boundTabIds = new Set(Object.values(bindings).map((value) => value?.tabId).filter(Boolean));
  return tabs.find((tab) => tab.id && !boundTabIds.has(tab.id) && isNewConversationUrl(tab.url));
}

async function findConversationTab(target) {
  const conversationId = conversationIdFromUrl(target);
  if (!conversationId) return null;
  const key = conversationKey(conversationId);
  const stored = await chrome.storage.session.get(key);
  const bound = stored[key];
  if (bound?.tabId) {
    const tab = await safeTab(bound.tabId);
    if (tab && sameConversationUrl(tab.url, target)) return tab;
  }
  const tabs = await chatGptTabs();
  const discovered = tabs.find((tab) => tab.id && sameConversationUrl(tab.url, target));
  if (discovered?.id) await bindConversationTab(conversationId, discovered.id);
  return discovered || null;
}

async function bindConversationTab(conversationId, tabId) {
  if (!conversationId || !tabId) return;
  await chrome.storage.session.set({ [conversationKey(conversationId)]: { tabId } });
}

async function conversationBindings() {
  const stored = await chrome.storage.session.get(null);
  return Object.fromEntries(Object.entries(stored).filter(([key]) => key.startsWith(CONVERSATION_PREFIX)));
}

async function chatGptTabs() {
  return chrome.tabs.query({ url: 'https://chatgpt.com/*' });
}

async function safeTab(tabId) {
  try { return await chrome.tabs.get(tabId); }
  catch { return null; }
}

async function releaseRequest(requestId) {
  await chrome.storage.session.remove(requestKey(requestId));
}

async function sendToChatGpt(tabId, payload) {
  let lastError;
  for (let attempt = 0; attempt < 12; attempt++) {
    try {
      const response = await chrome.tabs.sendMessage(tabId, payload);
      if (response?.ok) return;
      lastError = new Error(response?.error || 'ChatGPT content script chưa sẵn sàng.');
    } catch (error) { lastError = error; }
    await delay(250);
  }
  throw lastError || new Error('Không thể kết nối content script trên chatgpt.com.');
}

async function waitForTab(tabId) {
  const current = await chrome.tabs.get(tabId);
  if (current.status === 'complete') return;
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => finish(new Error('ChatGPT tải trang quá lâu.')), 20_000);
    const listener = (changedId, info) => { if (changedId === tabId && info.status === 'complete') finish(); };
    const finish = (error) => {
      clearTimeout(timer); chrome.tabs.onUpdated.removeListener(listener);
      if (error) reject(error); else resolve();
    };
    chrome.tabs.onUpdated.addListener(listener);
  });
}

async function postJson(baseUrl, path, body) {
  const response = await fetch(`${baseUrl}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-ChatCmdClient': 'chatgpt-extension' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    let message = `ChatCMD local API trả lỗi ${response.status}.`;
    try { message = (await response.json()).detail || message; } catch { /* non-json error */ }
    throw new Error(message);
  }
  return response.status === 204 ? undefined : response.json();
}

async function requestContext(requestId) {
  const key = requestKey(requestId);
  const value = await chrome.storage.session.get(key);
  return value[key];
}

function requestKey(requestId) { return `${REQUEST_PREFIX}${requestId}`; }
function conversationKey(conversationId) { return `${CONVERSATION_PREFIX}${conversationId}`; }
function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi ChatGPT bridge.'); }

function conversationTarget(value) {
  if (!value) return CHATGPT_HOME;
  const url = new URL(value);
  if (url.origin !== 'https://chatgpt.com') throw new Error('Conversation URL không thuộc chatgpt.com.');
  return url.href;
}

function conversationIdFromUrl(value) {
  try { return new URL(value).pathname.match(/^\/c\/([^/?#]+)/)?.[1] || null; }
  catch { return null; }
}

function sameConversationUrl(left, right) {
  const leftId = conversationIdFromUrl(left || '');
  const rightId = conversationIdFromUrl(right || '');
  return Boolean(leftId && rightId && leftId === rightId);
}

function isNewConversationUrl(value) {
  try {
    const url = new URL(value || '');
    return url.origin === 'https://chatgpt.com' && url.pathname === '/';
  } catch { return false; }
}

function localOrigin(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' || !['localhost', '127.0.0.1'].includes(url.hostname)) throw new Error('ChatCMD bridge chỉ cho phép local HTTP origin.');
  return url.origin;
}
