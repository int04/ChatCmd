// The service worker owns the local endpoint and validates the sender's actual tab.
// No task, agent, permission or localBaseUrl supplied by a ChatGPT page is trusted.
const captureCapabilities = new Map();
async function handleNativeTurn(message, sender) {
  const tabId = sender.tab?.id;
  if (!tabId || (sender.frameId !== undefined && sender.frameId !== 0)) throw new Error('Native capture requires the top-level ChatGPT tab.');
  const tab = await safeTab(tabId);
  const id = conversationIdFromUrl(tab?.url || '');
  if (!id || id !== message.conversationId || conversationIdFromUrl(message.conversationUrl || '') !== id) {
    throw new Error('The ChatGPT conversation changed before capture.');
  }
  const bindings = await conversationBindings();
  const binding = bindings[conversationKey(id)];
  const localBaseUrl = localOrigin(binding?.localBaseUrl || approvalBaseUrl);
  const last = captureCapabilities.get(localBaseUrl) || 0;
  if (Date.now() - last > 30000) {
    let capabilities;
    try { capabilities = await getJson(localBaseUrl, '/api/local/chatgpt/capture/capabilities'); }
    catch { throw new Error('ChatCMD chưa hỗ trợ capture v2 hoặc đang offline. Hãy chạy bản ứng dụng mới và kiểm tra địa chỉ local API.'); }
    if (capabilities?.provider !== 'chatcmd' || capabilities.captureProtocol !== 2) throw new Error('ChatCMD capture protocol không tương thích; cần cập nhật ứng dụng.');
    captureCapabilities.set(localBaseUrl, Date.now());
  }
  const request = await postJson(localBaseUrl, '/api/local/chatgpt/capture/turns', {
    conversationId: id, conversationUrl: tab.url,
    userMessageId: message.userMessageId, content: message.content,
  });
  if (!request?.id || request.conversationId !== id) throw new Error('Invalid native capture acknowledgement.');
  // A late HTTP result may be retained in SQLite, but cannot claim the tab after navigation.
  if (conversationIdFromUrl((await safeTab(tabId))?.url || '') !== id) throw new Error('ChatGPT tab navigated during capture enrollment.');
  await chrome.storage.session.set({ [requestKey(request.id)]: { tabId, localBaseUrl, conversationUrl: tab.url } });
  await bindConversationTab(id, tabId, { requestId: request.id, localBaseUrl });
  return { request };
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === 'chatcmd-chatgpt-native-turn') {
    void handleNativeTurn(message, sender).then((value) => sendResponse({ ok: true, ...value }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message?.type === 'chatcmd-capture-diagnostic') {
    if (!sender.tab?.id || !isChatGptUrl(sender.tab.url)) return false;
    if (message.state === 'error') void logExtension('warn', 'capture', String(message.detail || '').slice(0, 500));
    sendResponse({ ok: true });
  }
  return false;
});
