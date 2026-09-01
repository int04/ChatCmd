const MAX_AUTO_RETRIES = 2;
const RAW_BUBBLE_STABILITY_MS = 1_200;
const SILENT_RETRY_GRACE_MS = 8_000;
const ERROR_INTERRUPT_GRACE_MS = 2_500;
const COMPLETION_PING_INTERVAL_MS = 1_000;
const INTERRUPTED_PROGRESS_PROMPT = 'Tôi vừa bị gián đoạn kết nối. Vui lòng kiểm tra trạng thái công việc ở lượt trước. Nếu chưa hoàn tất, hãy tiếp tục từ trạng thái hiện tại và hoàn thành phần còn lại; không làm lại những phần đã xong. Nếu đã hoàn tất, hãy trả lại kết quả cuối.';
const {
  assistantNodes, clickStopButton, findSendButton, findStopButton, findThreadError,
  findVisible, isVisible, latestMessageText, normalize,
} = globalThis.ChatCmdConversationDom;

let activeRequest = null;
let reconcileScheduled = false;

void chrome.runtime.sendMessage({ type: 'chatcmd-return-binding-status' }, (response) => {
  if (response?.ok && response.enabled) renderReturnToChatCmd(true);
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'chatcmd-chatgpt-run') {
    const composer = findComposer();
    if (!composer || findStopButton()) {
      sendResponse({ ok: false, error: 'Tab ChatGPT chưa sẵn sàng nhận tin nhắn mới.' });
      return false;
    }
    if (activeRequest && Date.now() - activeRequest.startedAt < 1_500) {
      sendResponse({ ok: false, error: 'Tab ChatGPT này đang xử lý một yêu cầu khác.' });
      return false;
    }
    activeRequest = { id: message.requestId, stopRequested: false, retryCount: 0, resultReported: false, startedAt: Date.now() };
    void runRequest(message).finally(() => {
      if (activeRequest?.id === message.requestId) activeRequest = null;
    });
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === 'chatcmd-chatgpt-stop') {
    if (!activeRequest || activeRequest.id !== message.requestId) {
      sendResponse({ ok: false, error: 'Không tìm thấy lượt ChatGPT đang chạy trên tab này.' });
      return false;
    }
    activeRequest.stopRequested = true;
    clickStopButton();
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === 'chatcmd-chatgpt-ready') {
    const composer = findComposer();
    const generating = Boolean(findStopButton());
    sendResponse({
      ok: true,
      ready: Boolean(composer) && !generating,
      composerReady: Boolean(composer),
      generating,
    });
    return false;
  }
  if (message?.type === 'chatcmd-return-binding') {
    renderReturnToChatCmd(message.enabled !== false);
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === 'chatcmd-chatgpt-reconcile') {
    void reconcileActiveRequest(message.requestId)
      .then((result) => sendResponse({ ok: true, ...result }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  return false;
});

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') scheduleActiveRequestReconcile();
});
window.addEventListener('focus', scheduleActiveRequestReconcile);

async function runRequest(message) {
  let started = false;
  let conversationId;
  let conversationUrl;
  try {
    const composer = await waitForComposer();
    await selectModel(message.model);
    const assistantCount = assistantNodes().length;
    setComposerText(composer, message.submittedContent);
    await submitPrompt(composer);
    ({ conversationId, conversationUrl } = await waitForConversationIdentity());
    started = true;
    await progress({
      requestId: message.requestId,
      stage: 'started',
      conversationId,
      conversationUrl,
      model: message.model || 'Auto',
      userText: latestMessageText('user') || message.submittedContent,
    });
    const result = await waitForAssistant(assistantCount, message.requestId, message.submittedContent);
    const finalIdentity = currentConversationIdentity();
    if (finalIdentity && !isProvisionalConversationId(finalIdentity.conversationId)) {
      conversationId = finalIdentity.conversationId;
      conversationUrl = finalIdentity.conversationUrl;
    }
    await reportRequestResult({
      requestId: message.requestId,
      status: activeRequest?.id === message.requestId && activeRequest.stopRequested ? 'stopped' : 'completed',
      conversationId,
      conversationUrl: conversationUrl || window.location.href,
      assistantContent: result,
    });
  } catch (error) {
    await reportRequestResult({
      requestId: message.requestId,
      status: activeRequest?.id === message.requestId && activeRequest.stopRequested ? 'stopped' : 'failed',
      conversationId,
      conversationUrl: conversationUrl || (started ? window.location.href : undefined),
      assistantContent: started ? latestMessageText('assistant') : undefined,
      errorMessage: errorMessage(error),
    });
  }
}

function scheduleActiveRequestReconcile() {
  if (reconcileScheduled || !activeRequest?.id) return;
  reconcileScheduled = true;
  queueMicrotask(() => {
    reconcileScheduled = false;
    if (activeRequest?.id) void reconcileActiveRequest(activeRequest.id).catch(() => undefined);
  });
}

async function reconcileActiveRequest(requestId) {
  if (!activeRequest || activeRequest.id !== requestId) return { reconciled: false, reason: 'active_request_missing' };
  if (activeRequest.resultReported) return { reconciled: true, reason: 'result_already_reported' };
  const state = await requestState(requestId);
  if (!state.known) return { reconciled: false, reason: 'request_state_unknown' };
  if (!state.hasFinalResponse || !isTerminalRequestState(state)) return { reconciled: false, reason: state.active ? 'response_not_final' : 'request_not_active' };
  const assistantContent = latestMessageText('assistant');
  const identity = currentConversationIdentity();
  await reportRequestResult({
    requestId,
    status: activeRequest.stopRequested ? 'stopped' : 'completed',
    conversationId: identity?.conversationId,
    conversationUrl: identity?.conversationUrl || window.location.href,
    assistantContent: assistantContent || undefined,
  });
  return { reconciled: true, reason: assistantContent ? 'final_response_synced' : 'final_response_without_dom_text' };
}

async function reportRequestResult(payload) {
  if (activeRequest?.id === payload.requestId) {
    if (activeRequest.resultReported) return;
    activeRequest.resultReported = true;
  }
  await progress({ requestId: payload.requestId, stage: 'result', ...payload });
}

async function waitForComposer() {
  return waitFor(() => findComposer(), 20_000, composerMissingMessage());
}

function findComposer() {
  const direct = findUsableComposer(document, [
    '#prompt-textarea',
    '[data-testid="prompt-textarea"]',
    'textarea[name="prompt-textarea"]',
    'form textarea',
    'form [role="textbox"]',
    'form .ProseMirror[contenteditable="true"]',
    'form [contenteditable="plaintext-only"]',
    'form [contenteditable="true"]',
    '[role="textbox"][contenteditable="true"]',
    '[contenteditable="plaintext-only"]',
  ]);
  return direct || findComposerNearSendButton();
}

function findComposerNearSendButton() {
  const sendButton = findVisible([
    'button[data-testid="send-button"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
    'button[aria-label*="send" i]',
  ]);
  if (!sendButton) return null;

  const scopes = [];
  const form = sendButton.closest('form');
  if (form) scopes.push(form);
  let parent = sendButton.parentElement;
  for (let depth = 0; parent && depth < 6; depth += 1, parent = parent.parentElement) {
    if (!scopes.includes(parent)) scopes.push(parent);
  }

  const genericSelectors = [
    'textarea',
    '[role="textbox"]',
    '.ProseMirror[contenteditable]',
    '[contenteditable="plaintext-only"]',
    '[contenteditable="true"]',
  ];
  for (const scope of scopes) {
    const composer = findUsableComposer(scope, genericSelectors);
    if (composer) return composer;
  }
  return null;
}

function findUsableComposer(root, selectors) {
  for (const selector of selectors) {
    for (const element of root.querySelectorAll(selector)) {
      if (isUsableComposer(element)) return element;
    }
  }
  return null;
}

function isUsableComposer(element) {
  if (!isVisible(element)) return false;
  if (element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement) return !element.disabled;
  return element.getAttribute('aria-disabled') !== 'true' && element.getAttribute('contenteditable') !== 'false';
}

function composerMissingMessage() {
  const path = `${window.location.pathname}${window.location.search}` || '/';
  return `Không tìm thấy ô nhập ChatGPT trên ${path}. Hãy kiểm tra tab ChatGPT đầu tiên đang ở giao diện chat và bạn đã đăng nhập.`;
}

async function selectModel(model) {
  const target = String(model || '').trim();
  if (!target || ['auto', 'default', 'mặc định'].includes(target.toLowerCase())) return;
  const button = findModelSwitcherButton();
  if (!button) throw new Error(`ChatGPT hiện không hiển thị bộ chọn model cụ thể. Hãy dùng Auto trên giao diện ChatCMD.`);
  button.click();
  await delay(250);
  const option = await waitFor(() => {
    const candidates = [...document.querySelectorAll('[role="menuitem"], [role="option"], [data-radix-collection-item]')]
      .filter((item) => isVisible(item) && !item.closest('form[data-type="unified-composer"]'));
    const wanted = normalize(target);
    return candidates.find((item) => normalize(item.textContent).includes(wanted)) || null;
  }, 4_000, `Không tìm thấy model “${target}” trong menu ChatGPT.`);
  option.click();
  await delay(200);
}


function findModelSwitcherButton() {
  const selectors = [
    'button[data-testid="model-switcher-dropdown-button"]',
    'button[aria-label*="model" i]',
    'button[id*="model" i]',
  ];
  for (const selector of selectors) {
    for (const button of document.querySelectorAll(selector)) {
      if (isVisible(button) && !button.closest('form[data-type="unified-composer"]')) return button;
    }
  }

  const header = document.querySelector('#page-header');
  if (!header) return null;
  return [...header.querySelectorAll('button[aria-haspopup="menu"]')].find((button) => {
    if (!isVisible(button) || button.closest('form[data-type="unified-composer"]')) return false;
    return looksLikeModelLabel(cleanModelLabel(button.textContent || button.getAttribute('aria-label') || ''));
  }) || null;
}


function looksLikeModelLabel(value) {
  const text = String(value || '').trim();
  if (!text) return false;
  return /^(?:GPT(?:[-\s]?[0-9][\w.-]*)?(?:\s+(?:Pro|Thinking|Instant|Mini))?|o[1-9](?:[-\s][\w.-]+)?)$/i.test(text);
}

function cleanModelLabel(value) {
  const text = String(value || '').replace(/\s+/g, ' ').trim();
  if (!text) return '';
  const stripped = text.replace(/^model\s*[:：-]?\s*/i, '').trim();
  const ignored = ['model', 'models', 'select model', 'choose model', 'chatgpt', 'suy luận', 'vừa', 'thinking', 'reasoning'];
  if (!stripped || ignored.includes(stripped.toLowerCase())) return '';
  return stripped;
}

function setComposerText(composer, text) {
  composer.focus();
  if (composer instanceof HTMLTextAreaElement || composer instanceof HTMLInputElement) {
    const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(composer), 'value')?.set;
    if (setter) setter.call(composer, text); else composer.value = text;
    composer.dispatchEvent(new Event('input', { bubbles: true }));
    composer.dispatchEvent(new Event('change', { bubbles: true }));
    return;
  }
  const selection = window.getSelection();
  const range = document.createRange();
  range.selectNodeContents(composer);
  selection?.removeAllRanges();
  selection?.addRange(range);
  const inserted = document.execCommand('insertText', false, text);
  selection?.removeAllRanges();
  if (inserted) return;
  composer.replaceChildren();
  const paragraph = document.createElement('p');
  paragraph.textContent = text;
  composer.appendChild(paragraph);
  composer.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
}

async function submitPrompt(composer) {
  await delay(100);
  const button = await waitFor(findSendButton, 5_000, 'Không tìm thấy nút gửi của ChatGPT.');
  if (button.disabled || button.getAttribute('aria-disabled') === 'true') {
    await waitFor(() => !button.disabled && button.getAttribute('aria-disabled') !== 'true' ? button : null, 4_000, 'Nút gửi ChatGPT đang bị vô hiệu hóa.');
  }
  button.click();
  composer.blur();
}

async function waitForConversationIdentity() {
  return waitFor(currentConversationIdentity, 15_000, 'ChatGPT chưa tạo conversation ID trên URL.');
}

function currentConversationIdentity() {
  const match = window.location.pathname.match(/(?:^|\/)c\/([^/?#]+)/);
  if (!match) return null;
  return {
    conversationId: decodeURIComponent(match[1]),
    conversationUrl: window.location.href,
  };
}

function isProvisionalConversationId(value) {
  return /^WEB:/i.test(String(value || ''));
}

async function waitForAssistant(previousCount, requestId, submittedContent) {
  let baselineCount = previousCount;
  let lastText = '';
  let stableSince = 0;
  let lastActivityAt = Date.now();
  let lastStateCheckAt = 0;
  let lastCompletionPingAt = 0;
  let lastRequestState = unknownRequestState();
  let observedProgress = false;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10 * 60_000) {
    if (!activeRequest || activeRequest.id !== requestId || activeRequest.resultReported) return latestMessageText('assistant');
    const now = Date.now();
    const nodes = assistantNodes();
    const latest = nodes.at(-1);
    const text = latest?.innerText?.trim() || latest?.textContent?.trim() || '';
    const stopButton = findStopButton();
    const threadError = findThreadError();

    if (now - lastStateCheckAt > 800) {
      lastStateCheckAt = now;
      lastRequestState = await requestState(requestId);
      if (lastRequestState.stopRequested && activeRequest?.id === requestId && !activeRequest.stopRequested) {
        activeRequest.stopRequested = true;
        clickStopButton();
        await delay(250);
        continue;
      }
    }

    if (activeRequest?.id === requestId && activeRequest.stopRequested) {
      clickStopButton();
      if (!findStopButton() && (lastRequestState.stopRequested || isTerminalRequestState(lastRequestState))) return text;
    }

    if (stopButton) {
      observedProgress = true;
      lastActivityAt = now;
    }
    const hasNewAssistantText = nodes.length > baselineCount && Boolean(text);
    if (hasNewAssistantText) observedProgress = true;
    if (hasNewAssistantText && !threadError) {
      if (text !== lastText) {
        lastText = text;
        stableSince = now;
        lastActivityAt = now;
      } else if (!stableSince) {
        stableSince = now;
      }

      const stableMs = stableSince ? now - stableSince : 0;
      if (!stopButton && stableMs >= RAW_BUBBLE_STABILITY_MS && isTerminalRequestState(lastRequestState)) return text;
      if (
        !stopButton && !threadError && findComposer() &&
        stableMs >= RAW_BUBBLE_STABILITY_MS && now - lastCompletionPingAt >= COMPLETION_PING_INTERVAL_MS
      ) {
        lastCompletionPingAt = now;
        if (await reportBrowserCompletion(requestId, text)) return text;
      }
    }

    if (!stopButton && findComposer()) {
      const idleMs = now - lastActivityAt;
      const reason = threadError && idleMs >= ERROR_INTERRUPT_GRACE_MS
        ? 'thread_error'
        : idleMs >= SILENT_RETRY_GRACE_MS && !hasNewAssistantText
          ? 'send_ready_without_final'
          : null;
      if (reason) {
        lastRequestState = await requestState(requestId);
        if (!lastRequestState.known || lastRequestState.hasFinalResponse || !lastRequestState.active) {
          await delay(350);
          continue;
        }
        if ((activeRequest?.retryCount || 0) >= MAX_AUTO_RETRIES) {
          throw new Error(`ChatGPT vẫn chưa có phản hồi cuối sau ${MAX_AUTO_RETRIES} lần tự động gửi lại.`);
        }
        baselineCount = nodes.length;
        lastText = '';
        stableSince = 0;
        await retryPrompt(requestId, observedProgress ? INTERRUPTED_PROGRESS_PROMPT : submittedContent, reason, observedProgress);
        lastActivityAt = Date.now();
        await delay(650);
        continue;
      }
    }
    await delay(350);
  }
  throw new Error('Quá lâu chưa nhận được phản hồi hoàn tất từ ChatGPT.');
}

async function requestState(requestId) {
  try {
    const response = await chrome.runtime.sendMessage({ type: 'chatcmd-chatgpt-request-status', requestId });
    if (response?.ok !== true || response.known !== true) return unknownRequestState();
    return {
      known: true,
      running: response.running === true,
      stopRequested: response.stopRequested === true,
      hasFinalResponse: response.hasFinalResponse === true,
      active: response.active === true,
    };
  } catch {
    return unknownRequestState();
  }
}

async function reportBrowserCompletion(requestId, assistantContent) {
  const identity = currentConversationIdentity();
  try {
    const response = await chrome.runtime.sendMessage({
      type: 'chatcmd-chatgpt-progress',
      stage: 'browser-completed',
      requestId,
      conversationId: identity?.conversationId,
      conversationUrl: identity?.conversationUrl || window.location.href,
      assistantContent,
    });
    if (response?.ok !== true || response.browserCompleted !== true || response.hasFinalResponse !== true) return false;
    if (activeRequest?.id === requestId) activeRequest.resultReported = true;
    return true;
  } catch (error) {
    console.warn('[ChatCMD bridge] Không thể xác nhận raw bubble với backend.', error);
    return false;
  }
}

async function retryPrompt(requestId, content, reason, continuesPreviousProgress) {
  const composer = await waitForComposer();
  const retryCount = (activeRequest?.retryCount || 0) + 1;
  setComposerText(composer, content);
  await submitPrompt(composer);
  if (activeRequest?.id === requestId) activeRequest.retryCount = retryCount;
  await progress({ requestId, stage: 'retrying', retryCount, reason, continuesPreviousProgress });
}

function unknownRequestState() { return { known: false, running: null, stopRequested: false, hasFinalResponse: false, active: null }; }
function isTerminalRequestState(state) { return state.known && state.active !== true && (state.hasFinalResponse || (!state.running && !state.stopRequested)); }

async function waitFor(factory, timeoutMs, message) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const value = factory();
    if (value) return value;
    if (activeRequest?.stopRequested) throw new Error('Đã dừng theo yêu cầu người dùng.');
    await delay(120);
  }
  throw new Error(message);
}

async function progress(payload) {
  try { await chrome.runtime.sendMessage({ type: 'chatcmd-chatgpt-progress', ...payload }); }
  catch (error) { console.warn('[ChatCMD bridge]', error); }
}

function renderReturnToChatCmd(enabled) {
  globalThis.ChatCmdConversationUi?.renderReturnToChatCmd(enabled);
}

function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi khi thao tác ChatGPT.'); }
