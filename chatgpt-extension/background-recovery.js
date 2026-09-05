const CONTENT_SCRIPT_PLANS = [
  {
    kind: 'chatgpt',
    matches: (url) => url.startsWith('https://chatgpt.com/'),
    files: ['content-runtime.js', 'content-chatgpt-ui.js', 'content-chatgpt-dom.js', 'content-chatgpt-approval-ui.js', 'content-chatgpt.js'],
  },
  {
    kind: 'chatcmd',
    matches: (url) => /^http:\/\/(?:localhost|127\.0\.0\.1)(?::\d+)?\//.test(url),
    files: ['content-runtime.js', 'content-chatcmd.js'],
  },
];
const RECOVERY_REQUEST_PREFIX = 'chatcmd-recovery-request:';

async function recoverContentScriptsOnStartup() {
  const tabs = await chrome.tabs.query({});
  for (const tab of tabs) {
    if (!tab?.id || !tab.url) continue;
    const plan = CONTENT_SCRIPT_PLANS.find((item) => item.matches(tab.url));
    if (!plan || await contentScriptAlive(tab.id, plan.kind)) continue;
    try {
      await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: plan.files });
      await logExtension('info', 'background', `Đã khôi phục content script ${plan.kind} trên tab ${tab.id} sau khi extension reload.`);
    } catch (error) {
      await logExtension('warn', 'background', `Không thể khôi phục content script ${plan.kind} trên tab ${tab.id}: ${errorMessage(error)}`);
    }
  }
}

async function contentScriptAlive(tabId, kind) {
  try {
    const response = await chrome.tabs.sendMessage(tabId, { type: 'chatcmd-content-alive', kind });
    return response?.ok === true && response.kind === kind;
  } catch {
    return false;
  }
}

async function rememberRecoveryRequest(requestId, context) {
  if (!requestId || !context?.tabId) return;
  await chrome.storage.local.set({ [`${RECOVERY_REQUEST_PREFIX}${requestId}`]: context });
}

async function forgetRecoveryRequest(requestId) {
  if (requestId) await chrome.storage.local.remove(`${RECOVERY_REQUEST_PREFIX}${requestId}`);
}

async function recoveryRequestContext(requestId) {
  const key = `${RECOVERY_REQUEST_PREFIX}${requestId}`;
  const stored = await chrome.storage.local.get(key);
  return stored[key];
}

async function recoverRequestIdentity(message) {
  const localBaseUrl = localOrigin(message.localBaseUrl);
  const requestId = String(message.requestId || '').trim();
  const submitted = normalizeIdentityText(message.submittedContent);
  if (!requestId || !submitted) throw new Error('Thiếu dữ liệu để khôi phục ChatGPT conversation identity.');

  const durable = await recoveryRequestContext(requestId);
  if (durable?.tabId) {
    const recovered = await recoverIdentityFromTab(durable.tabId, requestId, localBaseUrl);
    if (recovered) return recovered;
  }

  const exact = [];
  const textMatches = [];
  for (const tab of await chatGptTabs()) {
    if (!tab?.id || !conversationIdFromUrl(tab.url || '')) continue;
    try {
      const probe = await sendToChatGpt(tab.id, { type: 'chatcmd-chatgpt-identity-probe' }, { quiet: true });
      if (!probe?.conversationId || !probe?.conversationUrl) continue;
      const candidate = { tab, probe };
      if (probe.requestId === requestId) exact.push(candidate);
      else if (normalizeIdentityText(probe.userText) === submitted) textMatches.push(candidate);
    } catch { /* unrelated/stale ChatGPT tab */ }
  }
  const matches = exact.length ? exact : textMatches;
  if (matches.length !== 1) {
    const reason = matches.length ? 'ambiguous_match' : 'matching_tab_not_found';
    await logExtension('warn', 'recovery', `Không thể khôi phục request ${requestId}: ${reason}; exact=${exact.length}; text=${textMatches.length}.`);
    return { recovered: false, reason };
  }
  return persistRecoveredIdentity(matches[0].tab, matches[0].probe, requestId, localBaseUrl);
}

async function recoverIdentityFromTab(tabId, requestId, localBaseUrl) {
  const tab = await safeTab(tabId);
  if (!tab?.id || !conversationIdFromUrl(tab.url || '')) return null;
  try {
    const probe = await sendToChatGpt(tab.id, { type: 'chatcmd-chatgpt-identity-probe' }, { quiet: true });
    if (!probe?.conversationId || !probe?.conversationUrl) return null;
    return persistRecoveredIdentity(tab, probe, requestId, localBaseUrl);
  } catch {
    return null;
  }
}

async function persistRecoveredIdentity(tab, probe, requestId, localBaseUrl) {
  await postJson(localBaseUrl, `/api/local/chatgpt/bridge/${encodeURIComponent(requestId)}/identity`, {
    conversationId: probe.conversationId,
    conversationUrl: probe.conversationUrl,
  });
  await chrome.storage.session.set({ [requestKey(requestId)]: { localBaseUrl, tabId: tab.id, conversationUrl: probe.conversationUrl } });
  await bindConversationTab(probe.conversationId, tab.id, { requestId, localBaseUrl });
  await forgetRecoveryRequest(requestId);
  await logExtension('info', 'recovery', `Đã khôi phục request ${requestId} từ tab ${tab.id}.`);
  return { recovered: true, tabId: tab.id, tabUrl: probe.conversationUrl };
}

function normalizeIdentityText(value) {
  return String(value || '').replace(/\s+/g, ' ').trim();
}
