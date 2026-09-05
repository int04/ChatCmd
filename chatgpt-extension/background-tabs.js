async function chatGptTabStatus(conversationUrl, sourceTabId) {
  if (!conversationUrl) {
    const prepared = await preparedTabForSource(sourceTabId);
    const tab = prepared || await findAvailableNewConversationTab();
    return { chatGptTabOpen: Boolean(tab?.id), tabId: tab?.id, tabUrl: tab?.url };
  }
  const target = await conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  if (!tab?.id) {
    return { chatGptTabOpen: false, conversationTabOpen: false, conversationReady: false };
  }
  let ready = false;
  try {
    const response = await sendToChatGpt(tab.id, { type: 'chatcmd-chatgpt-ready' }, { quiet: true });
    ready = response?.composerReady === true && response?.generating !== true;
    if (!ready) {
      await logExtension(
        'warn',
        'ready-check',
        `Tab ${tab.id} chưa ready: composerReady=${response?.composerReady === true}, generating=${response?.generating === true}, ready=${response?.ready === true}.`,
      );
    }
  } catch (error) {
    await logExtension('warn', 'ready-check', `Không đọc được trạng thái ready của tab ${tab.id}: ${errorMessage(error)}`);
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

async function prepareNewConversationTab(sourceTabId) {
  if (!sourceTabId) throw new Error('Không xác định được tab ChatCMD hiện tại.');
  let tab = await preparedTabForSource(sourceTabId);
  if (tab?.id) {
    await chrome.tabs.update(tab.id, { active: true });
  } else {
    tab = await findAvailableNewConversationTab();
    if (tab?.id) await chrome.tabs.update(tab.id, { active: true });
    else tab = await chrome.tabs.create({ url: CHATGPT_HOME, active: true });
  }
  if (!tab?.id) throw new Error('Không thể mở tab ChatGPT để chọn model.');
  if (tab.windowId) await chrome.windows.update(tab.windowId, { focused: true });
  await chrome.storage.session.set({
    [`${PREPARED_TAB_PREFIX}${sourceTabId}`]: { tabId: tab.id },
    [`${RETURN_TAB_PREFIX}${tab.id}`]: { sourceTabId },
  });
  void sendToChatGpt(tab.id, { type: 'chatcmd-return-binding', enabled: true }, { quiet: true }).catch(() => undefined);
  await logExtension('info', 'background', `Đã chuẩn bị tab ChatGPT ${tab.id} cho tab ChatCMD ${sourceTabId}.`);
  return tab;
}

async function preparedTabForSource(sourceTabId) {
  if (!sourceTabId) return null;
  const key = `${PREPARED_TAB_PREFIX}${sourceTabId}`;
  const stored = await chrome.storage.session.get(key);
  const tabId = stored[key]?.tabId;
  if (!tabId) return null;
  const tab = await safeTab(tabId);
  if (tab?.id && isNewConversationUrl(tab.url)) return tab;
  await chrome.storage.session.remove(key);
  return null;
}

async function acquireNewConversationTab(sourceTabId) {
  let tab = await preparedTabForSource(sourceTabId);
  if (tab?.id && sourceTabId) {
    await chrome.storage.session.remove(`${PREPARED_TAB_PREFIX}${sourceTabId}`);
  } else {
    tab = await findAvailableNewConversationTab();
    if (!tab?.id) tab = await chrome.tabs.create({ url: CHATGPT_HOME, active: true });
  }
  if (!tab?.id) throw new Error('Không thể tự mở tab ChatGPT mới. Hãy kiểm tra quyền của extension rồi thử lại.');
  await waitForTab(tab.id);
  return tab;
}

async function openConversationTab(conversationUrl, sourceTabId) {
  const target = await conversationTarget(conversationUrl);
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
  const target = await conversationTarget(conversationUrl);
  const tab = await findConversationTab(target);
  if (!tab?.id) throw new Error('Tab ChatGPT của cuộc trò chuyện này không còn mở.');
  await bindReturnSource(tab.id, sourceTabId);
  await chrome.tabs.update(tab.id, { active: true });
  if (tab.windowId) await chrome.windows.update(tab.windowId, { focused: true });
}

async function closeConversationTab(conversationUrl) {
  const target = await conversationTarget(conversationUrl);
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
  const stored = await chrome.storage.session.get(null);
  const reservedTabIds = new Set(
    Object.entries(stored)
      .filter(([key]) => key.startsWith(CONVERSATION_PREFIX) || key.startsWith(PREPARED_TAB_PREFIX) || key.startsWith(SUBAGENT_PREFIX))
      .map(([, value]) => value?.tabId)
      .filter(Boolean),
  );
  return tabs.find((tab) => tab.id && !reservedTabIds.has(tab.id) && isNewConversationUrl(tab.url));
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
      await refreshConversationAliases(tab.id, tab.url);
      return tab;
    }
    if (tab?.status === 'loading' && isChatGptUrl(tab.url)) return tab;
  }
  const tabs = await chatGptTabs();
  const discovered = tabs.find((tab) => tab.id && sameConversationUrl(tab.url, target));
  if (discovered?.id) await bindConversationTab(conversationId, discovered.id);
  return discovered || null;
}

async function bindConversationTab(conversationId, tabId, metadata = {}) {
  if (!conversationId || !tabId) return;
  const key = conversationKey(conversationId);
  const stored = await chrome.storage.session.get(key);
  const existing = stored[key] && typeof stored[key] === 'object' ? stored[key] : {};
  await chrome.storage.session.set({
    [key]: { ...existing, ...metadata, tabId },
  });
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
