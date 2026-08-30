const REQUEST_PREFIX = 'chatcmd-request:';
const CONVERSATION_PREFIX = 'chatcmd-conversation:';
const RETURN_TAB_PREFIX = 'chatcmd-return-tab:';
const LOG_KEY = 'chatcmd-extension-logs';
const MAX_LOGS = 200;
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
        void startRequest({ ...message, localBaseUrl, sourceTabId: sender.tab?.id }).catch((error) => void reportFailure(message.requestId, localBaseUrl, error));
        sendResponse({ ok: true });
      } catch (error) { sendResponse({ ok: false, error: errorMessage(error) }); }
      return false;
    }
    if (message.action === 'open-tab') {
      void openConversationTab(message.conversationUrl, sender.tab?.id).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'focus-tab') {
      void focusConversationTab(message.conversationUrl, sender.tab?.id).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'close-tab') {
      void closeConversationTab(message.conversationUrl).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'logs') {
      void extensionLogs().then((logs) => sendResponse({ ok: true, logs })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'clear-logs') {
      void chrome.storage.local.remove(LOG_KEY).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
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
  if (message.type === 'chatcmd-return-to-source') {
    void focusReturnSource(sender.tab?.id).then(() => sendResponse({ ok: true })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message.type === 'chatcmd-return-binding-status') {
    void hasReturnSource(sender.tab?.id).then((enabled) => sendResponse({ ok: true, enabled })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message.type === 'chatcmd-chatgpt-request-status') {
    void bridgeRequestState(message.requestId, sender.tab?.id)
      .then((state) => sendResponse({ ok: true, ...state }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message.type === 'chatcmd-chatgpt-recovery') {
    const reason = message.reason === 'thread_error' ? 'ChatGPT hiển thị lỗi luồng' : 'ChatGPT ngừng phản hồi nhưng composer đã sẵn sàng';
    void logExtension('warn', 'recovery', `${reason}; tự động tiếp tục lần ${Number(message.attempt) || 1} cho request ${message.requestId || 'unknown'}.`)
      .then(() => sendResponse({ ok: true }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  return false;
});

chrome.tabs.onRemoved.addListener((tabId) => {
  setTimeout(() => void handleClosedTab(tabId), 400);
});

chrome.tabs.onReplaced.addListener((addedTabId, removedTabId) => {
  void migrateTabBindings(removedTabId, addedTabId);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (!changeInfo.url || !isChatGptUrl(changeInfo.url)) return;
  void refreshConversationAliases(tabId, tab?.url || changeInfo.url);
});

async function startRequest(message) {
  if (!message.requestId || !message.submittedContent) throw new Error('Yêu cầu gửi ChatGPT không hợp lệ.');
  const target = conversationTarget(message.conversationUrl);
  const tab = message.conversationUrl
    ? await acquireConversationTab(target)
    : await acquireNewConversationTab();
  await bindReturnSource(tab.id, message.sourceTabId);
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
    if (message.conversationId && context.tabId) {
      await bindConversationTab(message.conversationId, context.tabId);
    }
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
    if (key === `${RETURN_TAB_PREFIX}${tabId}` || (key.startsWith(RETURN_TAB_PREFIX) && value?.sourceTabId === tabId)) {
      removals.push(key);
      continue;
    }
    if (!value || typeof value !== 'object' || value.tabId !== tabId) continue;
    if (key.startsWith(CONVERSATION_PREFIX)) removals.push(key);
    if (key.startsWith(REQUEST_PREFIX) && value.localBaseUrl) {
      const requestId = key.slice(REQUEST_PREFIX.length);
      failures.push(reportFailure(requestId, value.localBaseUrl, new Error('Tab ChatGPT liên kết với cuộc trò chuyện đã bị đóng. Mở lại cuộc trò chuyện ChatGPT để tiếp tục.')));
    }
  }
  if (removals.length) await chrome.storage.session.remove([...new Set(removals)]);
  await Promise.allSettled(failures);
}

async function migrateTabBindings(removedTabId, addedTabId) {
  if (!removedTabId || !addedTabId || removedTabId === addedTabId) return;
  const stored = await chrome.storage.session.get(null);
  const updates = {};
  const removals = [];
  for (const [key, value] of Object.entries(stored)) {
    if (!value || typeof value !== 'object') continue;
    if (key === `${RETURN_TAB_PREFIX}${removedTabId}`) {
      updates[`${RETURN_TAB_PREFIX}${addedTabId}`] = value;
      removals.push(key);
      continue;
    }
    let changed = false;
    const next = { ...value };
    if (value.tabId === removedTabId) {
      next.tabId = addedTabId;
      changed = true;
    }
    if (value.sourceTabId === removedTabId) {
      next.sourceTabId = addedTabId;
      changed = true;
    }
    if (changed) updates[key] = next;
  }
  if (Object.keys(updates).length) await chrome.storage.session.set(updates);
  if (removals.length) await chrome.storage.session.remove(removals);
  const tab = await safeTab(addedTabId);
  if (tab?.url) await refreshConversationAliases(addedTabId, tab.url);
  await logExtension('info', 'background', `Chrome thay tab ${removedTabId} bằng ${addedTabId}; đã chuyển binding ChatCMD sang tab mới.`);
}

async function refreshConversationAliases(tabId, tabUrl) {
  const liveId = conversationIdFromUrl(tabUrl || '');
  if (!tabId || !liveId) return;
  await bindConversationTab(liveId, tabId);
}

async function chatGptTabStatus(conversationUrl) {
  if (!conversationUrl) {
    const tab = await findAvailableNewConversationTab();
    return { chatGptTabOpen: Boolean(tab?.id), tabId: tab?.id, tabUrl: tab?.url };
  }
  const target = conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  if (!tab?.id) {
    return { chatGptTabOpen: false, conversationTabOpen: false, conversationReady: false };
  }
  let ready = false;
  try {
    const response = await sendToChatGpt(tab.id, { type: 'chatcmd-chatgpt-ready' }, { quiet: true });
    ready = response?.ready === true;
  } catch {
    ready = false;
  }
  return {
    chatGptTabOpen: true,
    conversationTabOpen: true,
    conversationReady: ready,
    tabId: tab.id,
    tabUrl: tab.url,
  };
}

async function acquireNewConversationTab() {
  let tab = await findAvailableNewConversationTab();
  if (!tab?.id) tab = await chrome.tabs.create({ url: CHATGPT_HOME, active: true });
  if (!tab?.id) throw new Error('Không thể tự mở tab ChatGPT mới. Hãy kiểm tra quyền của extension rồi thử lại.');
  await waitForTab(tab.id);
  return tab;
}

async function openConversationTab(conversationUrl, sourceTabId) {
  const target = conversationTarget(conversationUrl);
  const existing = await findConversationTab(target);
  if (existing?.id) {
    await bindReturnSource(existing.id, sourceTabId);
    return existing;
  }
  const tab = await chrome.tabs.create({ url: target, active: false });
  if (!tab?.id) throw new Error('Không thể mở tab ChatGPT của cuộc trò chuyện này.');
  const conversationId = conversationIdFromUrl(target);
  if (conversationId) await bindConversationTab(conversationId, tab.id);
  await waitForTab(tab.id);
  await bindReturnSource(tab.id, sourceTabId);
  return tab;
}

async function focusConversationTab(conversationUrl, sourceTabId) {
  const target = conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  if (!tab?.id) throw new Error('Tab ChatGPT của cuộc trò chuyện này không còn mở.');
  await bindReturnSource(tab.id, sourceTabId);
  await chrome.tabs.update(tab.id, { active: true });
  if (tab.windowId) await chrome.windows.update(tab.windowId, { focused: true });
}

async function closeConversationTab(conversationUrl) {
  const target = conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  if (!tab?.id) throw new Error('Tab ChatGPT của cuộc trò chuyện này không còn mở.');
  await chrome.tabs.remove(tab.id);
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
    if (tab && isChatGptUrl(tab.url) && isProvisionalConversationId(conversationId)) {
      const liveId = conversationIdFromUrl(tab.url);
      if (liveId && !isProvisionalConversationId(liveId)) {
        await bindConversationTab(liveId, tab.id);
      }
      return tab;
    }
    if (tab?.status === 'loading' && isChatGptUrl(tab.url)) return tab;
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

async function bindReturnSource(chatGptTabId, sourceTabId) {
  if (!chatGptTabId || !sourceTabId) return;
  await chrome.storage.session.set({ [`${RETURN_TAB_PREFIX}${chatGptTabId}`]: { sourceTabId } });
  await sendToChatGpt(chatGptTabId, { type: 'chatcmd-return-binding', enabled: true }, { quiet: true });
}

async function focusReturnSource(chatGptTabId) {
  if (!chatGptTabId) throw new Error('Không xác định được tab ChatGPT hiện tại.');
  const key = `${RETURN_TAB_PREFIX}${chatGptTabId}`;
  const stored = await chrome.storage.session.get(key);
  const sourceTabId = stored[key]?.sourceTabId;
  if (!sourceTabId) throw new Error('Không tìm thấy tab ChatCMD đã mở tab ChatGPT này.');
  const source = await safeTab(sourceTabId);
  if (!source?.id) throw new Error('Tab ChatCMD nguồn đã bị đóng.');
  await chrome.tabs.update(source.id, { active: true });
  if (source.windowId) await chrome.windows.update(source.windowId, { focused: true });
}

async function hasReturnSource(chatGptTabId) {
  if (!chatGptTabId) return false;
  const key = `${RETURN_TAB_PREFIX}${chatGptTabId}`;
  const stored = await chrome.storage.session.get(key);
  const sourceTabId = stored[key]?.sourceTabId;
  if (!sourceTabId) return false;
  return Boolean(await safeTab(sourceTabId));
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

async function sendToChatGpt(tabId, payload, options = {}) {
  let lastError;
  let reinjected = false;
  const quiet = options.quiet === true;
  if (!quiet) await logExtension('info', 'background', `Gửi ${payload?.type || 'message'} tới tab ${tabId}.`);
  for (let attempt = 0; attempt < 20; attempt++) {
    try {
      const response = await chrome.tabs.sendMessage(tabId, payload);
      if (response?.ok) {
        if (!quiet) await logExtension('info', 'background', `Tab ${tabId} phản hồi thành công ở lần ${attempt + 1}.`);
        return response;
      }
      lastError = new Error(response?.error || 'ChatGPT content script không thể hoàn tất yêu cầu.');
      await logExtension('error', 'content-chatgpt', `Tab ${tabId} đã nhận yêu cầu nhưng trả lỗi: ${errorMessage(lastError)}`);
      throw lastError;
    } catch (error) {
      lastError = error;
      if (!isMissingReceiverError(error)) {
        await logExtension('error', 'background', `Tab ${tabId} trả lỗi thực tế: ${errorMessage(error)}`);
        throw error;
      }
      await logExtension('warn', 'background', `Không có receiver ở tab ${tabId}, lần ${attempt + 1}: ${errorMessage(error)}`);
      if (!reinjected) {
        reinjected = true;
        try {
          await logExtension('info', 'background', `Inject lại content-chatgpt.js vào tab ${tabId}.`);
          await chrome.scripting.executeScript({ target: { tabId }, files: ['content-chatgpt.js'] });
          await logExtension('info', 'background', `Inject content-chatgpt.js vào tab ${tabId} thành công.`);
          await delay(150);
          continue;
        } catch (injectError) {
          lastError = injectError;
          await logExtension('error', 'background', `Inject tab ${tabId} thất bại: ${errorMessage(injectError)}`);
          throw injectError;
        }
      }
    }
    await delay(300);
  }
  await logExtension('error', 'background', `Không thể gửi tới tab ${tabId}: ${errorMessage(lastError)}`);
  throw lastError || new Error('Không thể kết nối content script trên chatgpt.com.');
}

async function logExtension(level, source, message) {
  const stored = await chrome.storage.local.get(LOG_KEY);
  const logs = Array.isArray(stored[LOG_KEY]) ? stored[LOG_KEY] : [];
  logs.push({ at: new Date().toISOString(), level, source, message: String(message || '') });
  await chrome.storage.local.set({ [LOG_KEY]: logs.slice(-MAX_LOGS) });
}

async function extensionLogs() {
  const stored = await chrome.storage.local.get(LOG_KEY);
  return Array.isArray(stored[LOG_KEY]) ? stored[LOG_KEY] : [];
}

function isMissingReceiverError(error) {
  const message = errorMessage(error).toLowerCase();
  return message.includes('receiving end does not exist') || message.includes('could not establish connection');
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

async function getJson(baseUrl, path) {
  const response = await fetch(`${baseUrl}${path}`, {
    method: 'GET',
    headers: { 'X-ChatCmdClient': 'chatgpt-extension' },
  });
  if (!response.ok) {
    let message = `ChatCMD local API trả lỗi ${response.status}.`;
    try { message = (await response.json()).detail || message; } catch { /* non-json error */ }
    throw new Error(message);
  }
  return response.json();
}

async function bridgeRequestState(requestId, tabId) {
  if (!requestId) return { running: false, stopRequested: false, hasFinalResponse: false, active: false };
  const context = await requestContext(requestId);
  if (!context?.localBaseUrl || !context?.tabId || context.tabId !== tabId) {
    return { running: false, stopRequested: false, hasFinalResponse: false, active: false };
  }
  const request = await getJson(context.localBaseUrl, `/api/local/chatgpt/requests/${encodeURIComponent(requestId)}`);
  const running = request?.status === 'running';
  const stopRequested = request?.status === 'stop_requested';
  const hasFinalResponse = request?.hasFinalResponse === true;
  return { running, stopRequested, hasFinalResponse, active: running && !hasFinalResponse };
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
  try {
    const url = new URL(value);
    if (url.origin !== 'https://chatgpt.com') return null;
    return url.pathname.match(/(?:^|\/)c\/([^/?#]+)/)?.[1] || null;
  } catch { return null; }
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

function isChatGptUrl(value) {
  try { return new URL(value || '').origin === 'https://chatgpt.com'; }
  catch { return false; }
}

function isProvisionalConversationId(value) {
  return /^WEB:/i.test(String(value || ''));
}

function localOrigin(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' || !['localhost', '127.0.0.1'].includes(url.hostname)) throw new Error('ChatCMD bridge chỉ cho phép local HTTP origin.');
  return url.origin;
}
