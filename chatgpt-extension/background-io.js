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
  if (payload?.type === 'chatcmd-chatgpt-run') {
    try {
      const health = await chrome.tabs.sendMessage(tabId, { type: 'chatcmd-content-alive', kind: 'chatgpt' });
      if (health?.ok && (health.captureProtocol !== 2 || health.renderProtocol !== 1 || !health.captureReady)) {
        throw new Error('Tab ChatGPT đang dùng content script cũ hoặc thiếu bộ capture. Hãy reload extension 0.1.6 và tải lại tab ChatGPT.');
      }
    } catch (error) { if (!isMissingReceiverError(error)) throw error; }
  }
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
          await logExtension('info', 'background', `Inject lại các ChatGPT content scripts vào tab ${tabId}.`);
          await injectChatGptScripts(tabId);
          await logExtension('info', 'background', `Inject ChatGPT content scripts vào tab ${tabId} thành công.`);
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

async function waitForChatGptReady(tabId) {
  let stableChecks = 0;
  let lastError;
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      const response = await sendToChatGpt(tabId, { type: 'chatcmd-chatgpt-ready' }, { quiet: true });
      if (response?.composerReady === true && response?.generating !== true) {
        stableChecks += 1;
        if (stableChecks >= 3) return;
      } else {
        stableChecks = 0;
      }
    } catch (error) {
      lastError = error;
      stableChecks = 0;
    }
    await delay(200);
  }
  throw lastError || new Error('Ô nhập ChatGPT chưa sẵn sàng sau khi mở trang dự án.');
}

async function postJson(baseUrl, path, body) {
  const response = await fetch(`${baseUrl}${path}`, {
    method: 'POST',
    signal: AbortSignal.timeout(10_000),
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
    signal: AbortSignal.timeout(10_000),
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
  if (!requestId) return { known: false, running: null, stopRequested: false, hasFinalResponse: false, active: null };
  const context = await requestContext(requestId);
  if (!context?.localBaseUrl || !context?.tabId || context.tabId !== tabId) {
    return { known: false, running: null, stopRequested: false, hasFinalResponse: false, active: null };
  }
  if (context.mode === 'subagent') {
    return { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true };
  }
  const request = await getJson(context.localBaseUrl, `/api/local/chatgpt/requests/${encodeURIComponent(requestId)}`);
  const running = request?.status === 'running';
  const stopRequested = request?.status === 'stop_requested';
  const hasFinalResponse = request?.hasFinalResponse === true;
  return { known: true, running, stopRequested, hasFinalResponse, active: running && !hasFinalResponse };
}

async function handleProgress(message, tabId) {
  if (!message.requestId) throw new Error('ChatGPT progress thiếu request ID.');
  const context = await requestContext(message.requestId);
  if (!context) throw new Error('Không tìm thấy ChatCMD request context.');
  if (tabId && context.tabId !== tabId) throw new Error('ChatGPT progress đến từ tab không khớp.');
  if (message.stage === 'observation') {
    if (!tabId || context.tabId !== tabId) throw new Error('Observation sender does not own this request.');
    if (context.mode === 'subagent') return { accepted: false };
    const tab = await safeTab(tabId);
    if (conversationIdFromUrl(tab?.url || '') !== message.conversationId) throw new Error('Observation conversation changed.');
    return postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/observation`, {
      conversationId: message.conversationId, conversationUrl: message.conversationUrl,
      userMessageId: message.userMessageId, revision: message.revision, messages: message.messages, completed: message.completed === true,
    });
  }
  const identity = await preferredConversationIdentity(context.tabId, message.conversationId, message.conversationUrl);
  if (message.stage === 'retrying') {
    await logExtension('warn', 'recovery', `Tự gửi lại request ${message.requestId}, lần ${Number(message.retryCount) || 1}, lý do ${message.reason || 'send_ready'}.`);
    return { stage: 'retrying' };
  }
  if (identity.conversationId && context.tabId) {
    await bindConversationTab(identity.conversationId, context.tabId, {
      requestId: message.requestId,
      localBaseUrl: context.localBaseUrl,
      mode: context.mode,
      subagentId: context.subagentId,
      attempt: context.attempt,
    });
  }
  if (context.mode === 'subagent') {
    if (message.stage === 'started') {
      await postJson(context.localBaseUrl, `/api/local/subagents/${encodeURIComponent(context.subagentId)}/fallback/started`, {
        attempt: context.attempt,
        conversationId: identity.conversationId,
        conversationUrl: identity.conversationUrl,
      });
      return { stage: 'started' };
    }
    if (message.stage === 'browser-completed' || message.stage === 'result') {
      const status = message.stage === 'browser-completed' ? 'completed' : (message.status || 'failed');
      const result = await postJson(context.localBaseUrl, `/api/local/subagents/${encodeURIComponent(context.subagentId)}/fallback/result`, {
        attempt: context.attempt,
        status,
        conversationId: identity.conversationId,
        conversationUrl: identity.conversationUrl,
        assistantContent: message.assistantContent,
        errorMessage: message.errorMessage,
      });
      await releaseRequest(message.requestId);
      await chrome.storage.session.remove(`${SUBAGENT_PREFIX}${context.subagentId}`);
      if (context.tabId) setTimeout(() => void safeTab(context.tabId).then((tab) => tab?.id && chrome.tabs.remove(tab.id).catch(() => undefined)), 100);
      return { stage: message.stage, completed: result?.completed === true, retryScheduled: result?.retryScheduled === true };
    }
    throw new Error(`ChatGPT sub-agent progress stage không được hỗ trợ: ${message.stage || 'missing'}.`);
  }
  if (message.stage === 'started') {
    await postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/started`, {
      conversationId: identity.conversationId, conversationUrl: identity.conversationUrl,
      model: message.model, userText: message.userText,
    });
    if (identity.conversationId && identity.conversationUrl) await forgetRecoveryRequest(message.requestId);
    return { stage: 'started' };
  }
  if (message.stage === 'browser-completed') {
    const result = await postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/browser-completed`, {
      conversationId: identity.conversationId, conversationUrl: identity.conversationUrl,
      assistantContent: message.assistantContent,
    });
    await releaseRequest(message.requestId);
    return { stage: 'browser-completed', browserCompleted: result?.status === 'completed', hasFinalResponse: result?.hasFinalResponse === true };
  }
  if (message.stage === 'result') {
    await postJson(context.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(message.requestId)}/result`, {
      status: message.status, conversationId: identity.conversationId,
      conversationUrl: identity.conversationUrl, assistantContent: message.assistantContent,
      errorMessage: message.errorMessage,
    });
    await releaseRequest(message.requestId);
    return { stage: 'result' };
  }
  throw new Error(`ChatGPT progress stage không được hỗ trợ: ${message.stage || 'missing'}.`);
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

async function conversationTarget(value) {
  if (!value) return CHATGPT_HOME;
  const url = new URL(value);
  if (url.origin !== 'https://chatgpt.com') throw new Error('Conversation URL không thuộc chatgpt.com.');
  const conversationId = conversationIdFromUrl(url.href);
  if (conversationId && isProvisionalConversationId(conversationId)) {
    const key = `${CONVERSATION_ALIAS_PREFIX}${conversationId}`;
    const stored = await chrome.storage.local.get(key);
    const aliasUrl = stored[key]?.conversationUrl;
    const aliasId = conversationIdFromUrl(aliasUrl || '');
    if (aliasId && !isProvisionalConversationId(aliasId)) return aliasUrl;
  }
  return url.href;
}

function conversationIdFromUrl(value) {
  try {
    const url = new URL(value);
    if (url.origin !== 'https://chatgpt.com') return null;
    const id = url.pathname.match(/(?:^|\/)c\/([^/?#]+)/)?.[1];
    return id ? decodeURIComponent(id) : null;
  } catch { return null; }
}

function sameConversationUrl(left, right) {
  const leftId = conversationIdFromUrl(left || '');
  const rightId = conversationIdFromUrl(right || '');
  return Boolean(leftId && rightId && leftId === rightId);
}

function normalizeNewConversationUrl(value) {
  if (!value) return CHATGPT_HOME;
  const url = new URL(value);
  if (url.origin !== 'https://chatgpt.com' || !/^\/g\/g-p-[A-Za-z0-9_-]+\/project$/.test(url.pathname) || url.search || url.hash) {
    throw new Error('Link dự án ChatGPT không đúng định dạng https://chatgpt.com/g/g-p-{MÃ}/project.');
  }
  return `${url.origin}${url.pathname}`;
}

function isNewConversationUrl(value, target = CHATGPT_HOME) {
  try {
    const url = new URL(value || '');
    const expected = new URL(target || CHATGPT_HOME);
    return url.origin === expected.origin && url.pathname === expected.pathname && !url.search && !url.hash;
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
