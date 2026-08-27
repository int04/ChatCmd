const REQUEST_PREFIX = 'chatcmd-request:';
const CHATGPT_HOME = 'https://chatgpt.com/';

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message || typeof message !== 'object') return false;
  if (message.type === 'chatcmd-local-command') {
    if (message.action === 'ping') {
      void hasChatGptTab().then((chatGptTabOpen) => sendResponse({ ok: true, chatGptTabOpen }))
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

async function startRequest(message) {
  if (!message.requestId || !message.submittedContent) throw new Error('Yêu cầu gửi ChatGPT không hợp lệ.');
  const target = conversationTarget(message.conversationUrl);
  const acquired = await acquireChatGptTab(target);
  await chrome.storage.session.set({
    [requestKey(message.requestId)]: {
      localBaseUrl: message.localBaseUrl,
      tabId: acquired.tab.id,
      closeWhenDone: false,
    },
  });
  await sendToChatGpt(acquired.tab.id, {
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
    await releaseRequest(message.requestId, context);
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
  await releaseRequest(requestId, await requestContext(requestId));
}

async function hasChatGptTab() {
  const tabs = await chrome.tabs.query({ url: 'https://chatgpt.com/*' });
  return tabs.some((tab) => Boolean(tab.id));
}

async function acquireChatGptTab(target) {
  const tabs = await chrome.tabs.query({ url: 'https://chatgpt.com/*' });
  let tab = tabs[0];
  if (!tab?.id) throw new Error('Không có tab ChatGPT nào đang mở. Hãy mở chatgpt.com rồi thử lại.');
  tab = await chrome.tabs.update(tab.id, { url: target });
  if (!tab.id) throw new Error('Không tìm thấy tab ChatGPT để gửi yêu cầu.');
  await waitForTab(tab.id);
  return { tab };
}

async function releaseRequest(requestId, context) {
  await chrome.storage.session.remove(requestKey(requestId));
  if (!context?.closeWhenDone || !context.tabId) return;
  try { await chrome.tabs.remove(context.tabId); }
  catch { /* tab may already be closed by the user */ }
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
function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi ChatGPT bridge.'); }

function conversationTarget(value) {
  if (!value) return CHATGPT_HOME;
  const url = new URL(value);
  if (url.origin !== 'https://chatgpt.com') throw new Error('Conversation URL không thuộc chatgpt.com.');
  return url.href;
}

function localOrigin(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' || !['localhost', '127.0.0.1'].includes(url.hostname)) throw new Error('ChatCMD bridge chỉ cho phép local HTTP origin.');
  return url.origin;
}
