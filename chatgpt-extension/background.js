const REQUEST_PREFIX = 'chatcmd-request:';
const CONVERSATION_PREFIX = 'chatcmd-conversation:';
const CONVERSATION_ALIAS_PREFIX = 'chatcmd-conversation-alias:';
const RETURN_TAB_PREFIX = 'chatcmd-return-tab:';
const PREPARED_TAB_PREFIX = 'chatcmd-prepared-tab:';
const SUBAGENT_PREFIX = 'chatcmd-subagent:';
const LOG_KEY = 'chatcmd-extension-logs';
const MAX_LOGS = 200;
const CHATGPT_HOME = 'https://chatgpt.com/';

importScripts('background-io.js', 'background-tabs.js', 'approval-bridge.js');

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message || typeof message !== 'object') return false;
  if (message.type === 'chatcmd-approval-state-request') {
    void approvalBridgeState()
      .then((state) => sendResponse({ ok: true, ...state }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message.type === 'chatcmd-approval-decision') {
    void resolveGlobalApproval(message)
      .then((result) => sendResponse({ ok: true, ...result }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message.type === 'chatcmd-local-command') {
    if (message.localBaseUrl) void configureApprovalBridge(message.localBaseUrl).catch(() => undefined);
    if (message.action === 'ping') {
      void chatGptTabStatus(message.conversationUrl, sender.tab?.id)
        .then((status) => sendResponse({ ok: true, ...status }))
        .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
    if (message.action === 'prepare-tab') {
      void prepareNewConversationTab(sender.tab?.id, message.newConversationUrl)
        .then((tab) => sendResponse({ ok: true, tabId: tab.id, tabUrl: tab.url }))
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
    if (message.action === 'subagent-send') {
      try {
        const localBaseUrl = localOrigin(message.localBaseUrl);
        void startSubagentRequest({ ...message, localBaseUrl })
          .then(() => sendResponse({ ok: true }))
          .catch(async (error) => {
            await reportSubagentFailure(message.subagentId, message.attempt, localBaseUrl, error);
            sendResponse({ ok: false, error: errorMessage(error) });
          });
      } catch (error) { sendResponse({ ok: false, error: errorMessage(error) }); }
      return true;
    }
    if (message.action === 'subagent-close') {
      void closeSubagentRequest(message.subagentId)
        .then(() => sendResponse({ ok: true }))
        .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
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
    if (message.action === 'reconcile') {
      void reconcileRequest(message.requestId)
        .then((result) => sendResponse({ ok: true, ...result }))
        .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
      return true;
    }
  }
  if (message.type === 'chatcmd-chatgpt-progress') {
    void handleProgress(message, sender.tab?.id).then((result) => sendResponse({ ok: true, ...result })).catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
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
  const target = await conversationTarget(message.conversationUrl);
  const tab = message.conversationUrl
    ? await acquireConversationTab(target)
    : await acquireNewConversationTab(message.sourceTabId, message.newConversationUrl);
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

async function startSubagentRequest(message) {
  if (!message.subagentId || !message.childTaskId || !message.submittedContent || !Number.isInteger(Number(message.attempt))) {
    throw new Error('Yêu cầu fallback sub-agent không hợp lệ.');
  }
  const attempt = Number(message.attempt);
  const subagentKey = `${SUBAGENT_PREFIX}${message.subagentId}`;
  const stored = await chrome.storage.session.get(subagentKey);
  const existing = stored[subagentKey];
  if (existing?.attempt === attempt && existing?.tabId && await safeTab(existing.tabId)) return;
  if (existing) await closeSubagentRequest(message.subagentId);

  const target = normalizeNewConversationUrl(message.newConversationUrl);
  const tab = await chrome.tabs.create({ url: target, active: false });
  if (!tab?.id) throw new Error('Không thể mở tab ChatGPT cho sub-agent.');
  await waitForTab(tab.id);
  const requestId = `subagent:${message.subagentId}:${attempt}`;
  await chrome.storage.session.set({
    [requestKey(requestId)]: {
      mode: 'subagent',
      localBaseUrl: message.localBaseUrl,
      tabId: tab.id,
      subagentId: message.subagentId,
      childTaskId: message.childTaskId,
      attempt,
      conversationUrl: null,
    },
    [subagentKey]: { requestId, tabId: tab.id, attempt },
  });
  await sendToChatGpt(tab.id, {
    type: 'chatcmd-chatgpt-run',
    requestId,
    submittedContent: message.submittedContent,
    model: message.model || 'Auto',
  });
}

async function closeSubagentRequest(subagentId) {
  if (!subagentId) return;
  const key = `${SUBAGENT_PREFIX}${subagentId}`;
  const stored = await chrome.storage.session.get(key);
  const binding = stored[key];
  if (!binding) return;
  const context = binding.requestId ? await requestContext(binding.requestId) : null;
  const tab = binding.tabId ? await safeTab(binding.tabId) : null;
  const conversationUrl = tab?.url || '';
  const conversationId = conversationIdFromUrl(conversationUrl);
  if (conversationId && context?.mode === 'subagent' && context.localBaseUrl && context.attempt) {
    try {
      await postJson(context.localBaseUrl, `/api/local/subagents/${encodeURIComponent(subagentId)}/fallback/started`, {
        attempt: Number(context.attempt),
        conversationId,
        conversationUrl,
      });
    } catch { /* the MCP claim already owns completion; identity sync is best effort */ }
  }
  if (binding.requestId) await releaseRequest(binding.requestId);
  await chrome.storage.session.remove(key);
  if (tab?.id) {
    try { await chrome.tabs.remove(tab.id); } catch { /* tab already closed */ }
  }
}

async function reportSubagentFailure(subagentId, attempt, localBaseUrl, error) {
  if (!subagentId || !attempt || !localBaseUrl) return;
  const key = `${SUBAGENT_PREFIX}${subagentId}`;
  const stored = await chrome.storage.session.get(key);
  const binding = stored[key];
  const requestId = binding?.requestId || `subagent:${subagentId}:${Number(attempt)}`;
  await releaseRequest(requestId);
  await chrome.storage.session.remove(key);
  if (binding?.tabId && await safeTab(binding.tabId)) {
    try { await chrome.tabs.remove(binding.tabId); } catch { /* tab already closed */ }
  }
  try {
    await postJson(localBaseUrl, `/api/local/subagents/${encodeURIComponent(subagentId)}/fallback/result`, {
      attempt: Number(attempt),
      status: 'failed',
      errorMessage: errorMessage(error),
    });
  } catch { /* the local app may already be closed */ }
}

async function stopRequest(message) {
  localOrigin(message.localBaseUrl);
  if (!message.requestId) throw new Error('Thiếu request ID cần dừng.');
  const context = await requestContext(message.requestId);
  if (!context?.tabId) throw new Error('Không tìm thấy tab ChatGPT đang xử lý yêu cầu này.');
  await chrome.tabs.sendMessage(context.tabId, { type: 'chatcmd-chatgpt-stop', requestId: message.requestId });
}

async function reconcileRequest(requestId) {
  if (!requestId) throw new Error('Thiếu request ID cần đồng bộ.');
  const context = await requestContext(requestId);
  if (!context?.tabId) return { reconciled: false, reason: 'request_context_missing' };
  const tab = await safeTab(context.tabId);
  if (!tab?.id) return { reconciled: false, reason: 'chatgpt_tab_missing' };
  const response = await sendToChatGpt(tab.id, {
    type: 'chatcmd-chatgpt-reconcile',
    requestId,
  }, { quiet: true });
  return {
    reconciled: response?.reconciled === true,
    reason: response?.reason,
  };
}

async function reportFailure(requestId, localBaseUrl, error) {
  if (!requestId) return;
  const context = await requestContext(requestId);
  if (context?.mode === 'subagent') {
    await reportSubagentFailure(context.subagentId, context.attempt, context.localBaseUrl || localBaseUrl, error);
    return;
  }
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
    if (
      key === `${RETURN_TAB_PREFIX}${tabId}` ||
      key === `${PREPARED_TAB_PREFIX}${tabId}` ||
      (key.startsWith(RETURN_TAB_PREFIX) && value?.sourceTabId === tabId)
    ) {
      removals.push(key);
      continue;
    }
    if (!value || typeof value !== 'object' || value.tabId !== tabId) continue;
    if (key.startsWith(PREPARED_TAB_PREFIX)) removals.push(key);
    if (key.startsWith(CONVERSATION_PREFIX)) removals.push(key);
    if (key.startsWith(SUBAGENT_PREFIX)) removals.push(key);
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
    if (key === `${PREPARED_TAB_PREFIX}${removedTabId}`) {
      updates[`${PREPARED_TAB_PREFIX}${addedTabId}`] = value;
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

async function preferredConversationIdentity(tabId, conversationId, conversationUrl) {
  const tab = tabId ? await safeTab(tabId) : null;
  const liveId = conversationIdFromUrl(tab?.url || '');
  if (liveId && !isProvisionalConversationId(liveId)) {
    return { conversationId: liveId, conversationUrl: tab.url };
  }
  return { conversationId, conversationUrl };
}

async function refreshConversationAliases(tabId, tabUrl) {
  const liveId = conversationIdFromUrl(tabUrl || '');
  if (!tabId || !liveId) return;
  if (isProvisionalConversationId(liveId)) {
    await bindConversationTab(liveId, tabId);
    return;
  }

  const bindings = await conversationBindings();
  let metadata = {};
  const provisionalKeys = [];
  for (const [key, binding] of Object.entries(bindings)) {
    if (!binding || binding.tabId !== tabId) continue;
    const boundId = key.slice(CONVERSATION_PREFIX.length);
    if (!isProvisionalConversationId(boundId)) continue;
    metadata = { ...metadata, ...binding };
    provisionalKeys.push(key);
    await chrome.storage.local.set({
      [`${CONVERSATION_ALIAS_PREFIX}${boundId}`]: {
        conversationId: liveId,
        conversationUrl: tabUrl,
      },
    });
    if (binding.requestId && binding.localBaseUrl) {
      try {
        if (binding.mode === 'subagent' && binding.subagentId && binding.attempt) {
          await postJson(binding.localBaseUrl, `/api/local/subagents/${encodeURIComponent(binding.subagentId)}/fallback/started`, {
            attempt: Number(binding.attempt),
            conversationId: liveId,
            conversationUrl: tabUrl,
          });
        } else {
          await postJson(binding.localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(binding.requestId)}/identity`, {
            conversationId: liveId,
            conversationUrl: tabUrl,
          });
        }
        await logExtension('info', 'background', `Đã nâng conversation ${boundId} thành ID thật ${liveId}.`);
      } catch (error) {
        await logExtension('warn', 'background', `Chưa đồng bộ được ChatGPT conversation ID thật ${liveId}: ${errorMessage(error)}`);
      }
    }
  }
  await bindConversationTab(liveId, tabId, metadata);
  if (provisionalKeys.length) await chrome.storage.session.remove(provisionalKeys);
}
