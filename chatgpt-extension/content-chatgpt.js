const AUTO_RECOVERY_PROMPT = 'Tôi vừa bị gián đoạn kết nối. Vui lòng kiểm tra trạng thái công việc ở lượt trước. Nếu chưa hoàn tất, hãy tiếp tục từ trạng thái hiện tại và hoàn thành phần còn lại; không làm lại những phần đã xong. Nếu đã hoàn tất, hãy trả lại kết quả cuối.';
const MAX_AUTO_RECOVERIES = 2;
const SILENT_RECOVERY_GRACE_MS = 8_000;
const ERROR_RECOVERY_GRACE_MS = 2_500;

let activeRequest = null;
let reconcileScheduled = false;

void chrome.runtime.sendMessage({ type: 'chatcmd-return-binding-status' }, (response) => {
  if (response?.ok && response.enabled) renderReturnToChatCmd(true);
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'chatcmd-chatgpt-run') {
    if (activeRequest) {
      sendResponse({ ok: false, error: 'Tab ChatGPT này đang xử lý một yêu cầu khác.' });
      return false;
    }
    activeRequest = { id: message.requestId, stopRequested: false, recoveryCount: 0, resultReported: false };
    void runRequest(message).finally(() => { activeRequest = null; });
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
    const generating = Boolean(activeRequest || findStopButton());
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
    const result = await waitForAssistant(assistantCount, message.requestId);
    const finalIdentity = currentConversationIdentity();
    if (finalIdentity && !isProvisionalConversationId(finalIdentity.conversationId)) {
      conversationId = finalIdentity.conversationId;
      conversationUrl = finalIdentity.conversationUrl;
    }
    await reportRequestResult({
      requestId: message.requestId,
      status: activeRequest?.stopRequested ? 'stopped' : 'completed',
      conversationId,
      conversationUrl: conversationUrl || window.location.href,
      assistantContent: result,
    });
  } catch (error) {
    await reportRequestResult({
      requestId: message.requestId,
      status: activeRequest?.stopRequested ? 'stopped' : 'failed',
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
  if (!state.hasFinalResponse) return { reconciled: false, reason: state.active ? 'response_not_final' : 'request_not_active' };
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
  const button = await waitFor(() => findVisible([
    'button[data-testid="send-button"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
  ]), 5_000, 'Không tìm thấy nút gửi của ChatGPT.');
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

async function waitForAssistant(previousCount, requestId) {
  let baselineCount = previousCount;
  let lastText = '';
  let stableSince = 0;
  let lastActivityAt = Date.now();
  let lastStateCheckAt = 0;
  let generationObserved = false;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10 * 60_000) {
    if (activeRequest?.resultReported) return latestMessageText('assistant');
    const now = Date.now();
    const nodes = assistantNodes();
    const latest = nodes.at(-1);
    const text = latest?.innerText?.trim() || latest?.textContent?.trim() || '';
    const stopButton = findStopButton();
    const threadError = findThreadError();
    if (stopButton || nodes.length > baselineCount || threadError) generationObserved = true;

    if (activeRequest?.stopRequested) {
      clickStopButton();
      if (!findStopButton()) return text;
    }

    if (!activeRequest?.stopRequested && now - lastStateCheckAt > 800) {
      lastStateCheckAt = now;
      const state = await requestState(requestId);
      if (state.stopRequested) {
        activeRequest.stopRequested = true;
        clickStopButton();
        await delay(250);
        continue;
      }
    }

    if (stopButton) lastActivityAt = now;
    if (nodes.length > baselineCount && text && !threadError) {
      if (text !== lastText) {
        lastText = text;
        stableSince = now;
        lastActivityAt = now;
      } else if (!stableSince) {
        stableSince = now;
      }

      if (!stopButton && stableSince && now - stableSince > 1_200 && now - lastStateCheckAt > 800) {
        lastStateCheckAt = now;
        const state = await requestState(requestId);
        if (state.hasFinalResponse || !state.running) return text;
      }
    }

    if (!stopButton && findComposer()) {
      const idleMs = now - lastActivityAt;
      const reason = threadError && idleMs >= ERROR_RECOVERY_GRACE_MS
        ? 'thread_error'
        : generationObserved && idleMs >= SILENT_RECOVERY_GRACE_MS
          ? 'silent_interrupt'
          : null;
      if (reason) {
        const state = await requestState(requestId);
        if (state.hasFinalResponse) {
          if (nodes.length > baselineCount && text) return text;
          await delay(350);
          continue;
        }
        if (!state.active) {
          if (nodes.length > baselineCount && text) return text;
          await delay(350);
          continue;
        }
        if ((activeRequest?.recoveryCount || 0) >= MAX_AUTO_RECOVERIES) {
          throw new Error(`ChatGPT vẫn bị gián đoạn sau ${MAX_AUTO_RECOVERIES} lần tự động tiếp tục.`);
        }
        baselineCount = nodes.length;
        lastText = '';
        stableSince = 0;
        await recoverInterruptedRequest(requestId, reason);
        lastActivityAt = Date.now();
        await delay(650);
        continue;
      }
    }
    await delay(350);
  }
  throw new Error('Quá lâu chưa nhận được phản hồi hoàn tất từ ChatGPT.');
}

function findThreadError() {
  const latestUser = [...document.querySelectorAll('[data-message-author-role="user"]')].filter(isVisible).at(-1);
  const latestAssistant = assistantNodes().at(-1);
  const candidates = [...document.querySelectorAll([
    'button[data-testid="regenerate-thread-error-button"]',
    '[class*="text-token-text-error"]',
    '[class*="bg-token-surface-error"]',
    '[class*="border-token-surface-error"]',
  ].join(','))].filter(isVisible);
  return candidates.reverse().find((element) => {
    const afterLatestUser = !latestUser || Boolean(latestUser.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING);
    if (!afterLatestUser) return false;
    if (!latestAssistant) return true;
    const assistantAfterError = Boolean(element.compareDocumentPosition(latestAssistant) & Node.DOCUMENT_POSITION_FOLLOWING);
    return !assistantAfterError;
  }) || null;
}

async function recoverInterruptedRequest(requestId, reason) {
  const composer = await waitForComposer();
  activeRequest.recoveryCount = (activeRequest.recoveryCount || 0) + 1;
  await recoveryEvent(requestId, reason, activeRequest.recoveryCount);
  setComposerText(composer, AUTO_RECOVERY_PROMPT);
  await submitPrompt(composer);
}

async function requestState(requestId) {
  try {
    const response = await chrome.runtime.sendMessage({ type: 'chatcmd-chatgpt-request-status', requestId });
    if (response?.ok !== true) return { running: false, stopRequested: false, hasFinalResponse: false, active: false };
    return {
      running: response.running === true,
      stopRequested: response.stopRequested === true,
      hasFinalResponse: response.hasFinalResponse === true,
      active: response.active === true,
    };
  } catch {
    return { running: false, stopRequested: false, hasFinalResponse: false, active: false };
  }
}

async function recoveryEvent(requestId, reason, attempt) {
  try { await chrome.runtime.sendMessage({ type: 'chatcmd-chatgpt-recovery', requestId, reason, attempt }); }
  catch (error) { console.warn('[ChatCMD recovery]', error); }
}

function assistantNodes() { return [...document.querySelectorAll('[data-message-author-role="assistant"]')].filter(isVisible); }
function latestMessageText(role) {
  const nodes = [...document.querySelectorAll(`[data-message-author-role="${role}"]`)].filter(isVisible);
  const latest = nodes.at(-1);
  return latest?.innerText?.trim() || latest?.textContent?.trim() || '';
}

function findStopButton() {
  return findVisible([
    'button[data-testid="stop-button"]',
    'button[data-testid="stop-generating-button"]',
    'button[aria-label*="Stop" i]',
  ]);
}
function clickStopButton() { const button = findStopButton(); if (button) button.click(); }

function findVisible(selectors) {
  for (const selector of selectors) {
    for (const element of document.querySelectorAll(selector)) if (isVisible(element)) return element;
  }
  return null;
}

function isVisible(element) {
  if (!(element instanceof Element)) return false;
  const rect = element.getBoundingClientRect();
  const style = getComputedStyle(element);
  return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
}

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
  renderChatCmdControlledState(enabled);
  const id = 'chatcmd-return-to-app';
  document.getElementById(id)?.remove();
  if (!enabled) return;

  const button = document.createElement('button');
  button.id = id;
  button.type = 'button';
  button.setAttribute('aria-label', 'Quay lại ChatCMD');
  button.title = 'Quay lại ChatCMD';
  button.innerHTML = `
    <span data-chatcmd-return-icon aria-hidden="true">↩</span>
    <span data-chatcmd-return-copy>
      <strong>Quay lại ChatCMD</strong>
      <small>Bấm để trở về</small>
    </span>
    <i data-chatcmd-return-dot aria-hidden="true"></i>
  `;

  Object.assign(button.style, {
    position: 'fixed', right: '24px', bottom: '112px', zIndex: '2147483647',
    display: 'grid', gridTemplateColumns: '46px minmax(0,1fr) 10px', alignItems: 'center', gap: '11px',
    minWidth: '220px', minHeight: '66px', padding: '9px 15px 9px 10px',
    border: '1px solid rgba(255,255,255,.28)', borderRadius: '20px',
    background: 'linear-gradient(135deg,rgba(124,58,237,.98),rgba(37,99,235,.98))', color: '#fff',
    boxShadow: '0 18px 46px rgba(76,29,149,.42),0 0 0 1px rgba(255,255,255,.08) inset',
    backdropFilter: 'blur(18px)', WebkitBackdropFilter: 'blur(18px)',
    font: '600 13px/1.2 system-ui,-apple-system,Segoe UI,sans-serif', cursor: 'pointer',
    transformOrigin: 'right center', transition: 'transform 180ms ease,box-shadow 180ms ease,filter 180ms ease',
    isolation: 'isolate', overflow: 'hidden'
  });

  const icon = button.querySelector('[data-chatcmd-return-icon]');
  const copy = button.querySelector('[data-chatcmd-return-copy]');
  const dot = button.querySelector('[data-chatcmd-return-dot]');
  Object.assign(icon.style, {
    width: '46px', height: '46px', display: 'grid', placeItems: 'center', borderRadius: '15px',
    background: 'rgba(255,255,255,.16)', boxShadow: '0 0 0 1px rgba(255,255,255,.14) inset',
    fontSize: '25px', fontWeight: '800'
  });
  Object.assign(copy.style, { minWidth: '0', display: 'grid', gap: '4px', textAlign: 'left' });
  Object.assign(copy.querySelector('strong').style, { fontSize: '14px', letterSpacing: '-.01em', whiteSpace: 'nowrap' });
  Object.assign(copy.querySelector('small').style, { color: 'rgba(255,255,255,.78)', fontSize: '11px', fontWeight: '550' });
  Object.assign(dot.style, {
    width: '9px', height: '9px', borderRadius: '50%', background: '#86efac',
    boxShadow: '0 0 0 4px rgba(134,239,172,.16),0 0 16px rgba(134,239,172,.8)'
  });

  button.animate([
    { boxShadow: '0 18px 46px rgba(76,29,149,.42),0 0 0 0 rgba(139,92,246,.34)' },
    { boxShadow: '0 20px 54px rgba(76,29,149,.52),0 0 0 12px rgba(139,92,246,0)' }
  ], { duration: 1900, iterations: Infinity, easing: 'ease-out' });
  icon.animate([
    { transform: 'translateX(0) rotate(0deg)' },
    { transform: 'translateX(-3px) rotate(-8deg)' },
    { transform: 'translateX(0) rotate(0deg)' }
  ], { duration: 1450, iterations: Infinity, easing: 'ease-in-out' });
  dot.animate([{ opacity: .55 }, { opacity: 1 }, { opacity: .55 }], { duration: 1100, iterations: Infinity, easing: 'ease-in-out' });
  button.animate([
    { opacity: 0, transform: 'translateX(26px) scale(.9)' },
    { opacity: 1, transform: 'translateX(0) scale(1.04)' },
    { opacity: 1, transform: 'translateX(0) scale(1)' }
  ], { duration: 520, easing: 'cubic-bezier(.16,1,.3,1)' });

  button.addEventListener('mouseenter', () => {
    button.style.transform = 'translateY(-4px) scale(1.035)';
    button.style.filter = 'brightness(1.1) saturate(1.08)';
    button.style.boxShadow = '0 24px 64px rgba(76,29,149,.55),0 0 0 1px rgba(255,255,255,.18) inset';
  });
  button.addEventListener('mouseleave', () => {
    button.style.transform = '';
    button.style.filter = '';
    button.style.boxShadow = '0 18px 46px rgba(76,29,149,.42),0 0 0 1px rgba(255,255,255,.08) inset';
  });
  button.addEventListener('click', () => {
    button.disabled = true;
    button.style.opacity = '.7';
    copy.querySelector('small').textContent = 'Đang quay lại…';
    chrome.runtime.sendMessage({ type: 'chatcmd-return-to-source' }, () => {
      button.disabled = false;
      button.style.opacity = '';
      copy.querySelector('small').textContent = 'Bấm để trở về';
    });
  });

  document.documentElement.appendChild(button);
}

function renderChatCmdControlledState(enabled) {
  const bannerId = 'chatcmd-controlled-banner';
  const frameId = 'chatcmd-controlled-frame';
  document.getElementById(bannerId)?.remove();
  document.getElementById(frameId)?.remove();
  if (!enabled) return;

  const banner = document.createElement('section');
  banner.id = bannerId;
  banner.setAttribute('role', 'status');
  banner.setAttribute('aria-live', 'polite');
  banner.innerHTML = `
    <span data-chatcmd-warning-icon aria-hidden="true">
      <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M10.3 2.9 1.8 17.1A2 2 0 0 0 3.5 20h17a2 2 0 0 0 1.7-2.9L13.7 2.9a2 2 0 0 0-3.4 0Z"/>
        <path d="M12 9v4"/><path d="M12 17h.01"/>
      </svg>
    </span>
    <span data-chatcmd-warning-copy>
      <strong>Đừng thao tác với tab ChatGPT này</strong>
      <span>Tab ChatGPT này đang được xử lý bởi ChatCMD, xin đừng thao tác trên tab trình duyệt này khi bạn vẫn đang sử dụng trên ChatCMD vì có thể gây lỗi cho bên ChatCMD. Chỉ đóng tab này nếu như bạn không còn sử dụng bên ChatCMD nữa.</span>
    </span>
    <span data-chatcmd-warning-state><i></i> ChatCMD đang sử dụng</span>
  `;
  Object.assign(banner.style, {
    position: 'fixed', left: '24px', top: '24px', zIndex: '2147483646',
    width: 'min(720px,calc(100vw - 48px))', boxSizing: 'border-box', display: 'grid',
    gridTemplateColumns: '52px minmax(0,1fr)', gap: '14px', alignItems: 'start', padding: '16px 18px',
    color: '#fff', border: '1px solid rgba(253,224,71,.54)', borderRadius: '22px',
    background: 'linear-gradient(135deg,rgba(15,23,42,.96),rgba(88,28,135,.94) 54%,rgba(124,45,18,.94))',
    boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 34px rgba(250,204,21,.18)',
    backdropFilter: 'blur(20px) saturate(1.25)', WebkitBackdropFilter: 'blur(20px) saturate(1.25)',
    font: '500 14px/1.55 system-ui,-apple-system,Segoe UI,sans-serif', pointerEvents: 'none', isolation: 'isolate', overflow: 'hidden'
  });
  const icon = banner.querySelector('[data-chatcmd-warning-icon]');
  const copy = banner.querySelector('[data-chatcmd-warning-copy]');
  const title = copy.querySelector('strong');
  const body = copy.querySelector('span');
  const state = banner.querySelector('[data-chatcmd-warning-state]');
  const dot = state.querySelector('i');
  Object.assign(icon.style, {
    width: '52px', height: '52px', display: 'grid', placeItems: 'center', borderRadius: '17px',
    color: '#fde68a', background: 'linear-gradient(135deg,rgba(245,158,11,.26),rgba(234,88,12,.2))',
    boxShadow: '0 0 0 1px rgba(253,224,71,.24) inset,0 0 24px rgba(245,158,11,.18)'
  });
  Object.assign(copy.style, { display: 'grid', gap: '6px', minWidth: '0', paddingRight: '4px' });
  Object.assign(title.style, { fontSize: '17px', lineHeight: '1.3', letterSpacing: '-.02em', fontWeight: '800', color: '#fff7ed' });
  Object.assign(body.style, { color: 'rgba(255,255,255,.8)', fontSize: '13px', lineHeight: '1.55' });
  Object.assign(state.style, {
    gridColumn: '2', justifySelf: 'start', display: 'inline-flex', alignItems: 'center', gap: '7px',
    marginTop: '2px', padding: '5px 9px', borderRadius: '999px', color: '#fef3c7',
    background: 'rgba(245,158,11,.12)', border: '1px solid rgba(253,224,71,.2)', fontSize: '11px', fontWeight: '700'
  });
  Object.assign(dot.style, { width: '7px', height: '7px', borderRadius: '50%', background: '#facc15', boxShadow: '0 0 0 4px rgba(250,204,21,.12)' });

  const frame = document.createElement('div');
  frame.id = frameId;
  frame.setAttribute('aria-hidden', 'true');
  Object.assign(frame.style, {
    position: 'fixed', inset: '0', zIndex: '2147483645', pointerEvents: 'none', boxSizing: 'border-box',
    border: '2px solid rgba(168,85,247,.95)', borderRadius: '2px',
    background: 'linear-gradient(to bottom,rgba(168,85,247,.17),transparent 13%),linear-gradient(to top,rgba(59,130,246,.15),transparent 13%),linear-gradient(to right,rgba(168,85,247,.14),transparent 11%),linear-gradient(to left,rgba(250,204,21,.12),transparent 11%)',
    boxShadow: '0 0 0 1px rgba(255,255,255,.12) inset,0 0 42px 12px rgba(168,85,247,.26) inset,0 0 90px 26px rgba(59,130,246,.12) inset'
  });

  banner.animate([
    { opacity: 0, transform: 'translateY(-18px) scale(.96)' },
    { opacity: 1, transform: 'translateY(2px) scale(1.012)', offset: .72 },
    { opacity: 1, transform: 'translateY(0) scale(1)' }
  ], { duration: 620, easing: 'cubic-bezier(.16,1,.3,1)' });
  banner.animate([
    { boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 22px rgba(250,204,21,.14)' },
    { boxShadow: '0 28px 92px rgba(15,23,42,.56),0 0 0 1px rgba(255,255,255,.12) inset,0 0 52px rgba(250,204,21,.34)' },
    { boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 22px rgba(250,204,21,.14)' }
  ], { duration: 2100, iterations: Infinity, easing: 'ease-in-out' });
  icon.animate([{ transform: 'scale(1)' }, { transform: 'scale(1.1)' }, { transform: 'scale(1)' }], { duration: 1250, iterations: Infinity, easing: 'ease-in-out' });
  dot.animate([{ opacity: .35, transform: 'scale(.8)' }, { opacity: 1, transform: 'scale(1.25)' }, { opacity: .35, transform: 'scale(.8)' }], { duration: 820, iterations: Infinity, easing: 'ease-in-out' });
  frame.animate([
    {
      borderColor: 'rgba(168,85,247,.6)',
      background: 'linear-gradient(to bottom,rgba(168,85,247,.1),transparent 11%),linear-gradient(to top,rgba(59,130,246,.09),transparent 11%),linear-gradient(to right,rgba(168,85,247,.08),transparent 9%),linear-gradient(to left,rgba(250,204,21,.07),transparent 9%)',
      boxShadow: '0 0 0 1px rgba(255,255,255,.08) inset,0 0 28px 7px rgba(168,85,247,.18) inset,0 0 60px 18px rgba(59,130,246,.08) inset'
    },
    {
      borderColor: 'rgba(250,204,21,.98)',
      background: 'linear-gradient(to bottom,rgba(250,204,21,.22),transparent 18%),linear-gradient(to top,rgba(168,85,247,.22),transparent 18%),linear-gradient(to right,rgba(59,130,246,.18),transparent 15%),linear-gradient(to left,rgba(250,204,21,.17),transparent 15%)',
      boxShadow: '0 0 0 1px rgba(255,255,255,.16) inset,0 0 58px 18px rgba(168,85,247,.4) inset,0 0 120px 38px rgba(59,130,246,.18) inset'
    },
    {
      borderColor: 'rgba(59,130,246,.9)',
      background: 'linear-gradient(to bottom,rgba(59,130,246,.18),transparent 15%),linear-gradient(to top,rgba(168,85,247,.2),transparent 16%),linear-gradient(to right,rgba(250,204,21,.13),transparent 12%),linear-gradient(to left,rgba(59,130,246,.16),transparent 13%)',
      boxShadow: '0 0 0 1px rgba(255,255,255,.12) inset,0 0 48px 14px rgba(59,130,246,.34) inset,0 0 105px 31px rgba(168,85,247,.15) inset'
    }
  ], { duration: 1650, iterations: Infinity, direction: 'alternate', easing: 'ease-in-out' });

  document.documentElement.append(frame, banner);
}

function normalize(value) { return String(value || '').replace(/\s+/g, ' ').trim().toLowerCase(); }
function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi khi thao tác ChatGPT.'); }
