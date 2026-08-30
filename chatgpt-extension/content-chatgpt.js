const AUTO_RECOVERY_PROMPT = 'Tôi vừa bị gián đoạn kết nối. Vui lòng kiểm tra trạng thái công việc ở lượt trước. Nếu chưa hoàn tất, hãy tiếp tục từ trạng thái hiện tại và hoàn thành phần còn lại; không làm lại những phần đã xong. Nếu đã hoàn tất, hãy trả lại kết quả cuối.';
const MAX_AUTO_RECOVERIES = 2;
const SILENT_RECOVERY_GRACE_MS = 8_000;
const ERROR_RECOVERY_GRACE_MS = 2_500;

let activeRequest = null;

void chrome.runtime.sendMessage({ type: 'chatcmd-return-binding-status' }, (response) => {
  if (response?.ok && response.enabled) renderReturnToChatCmd(true);
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'chatcmd-chatgpt-run') {
    if (activeRequest) {
      sendResponse({ ok: false, error: 'Tab ChatGPT này đang xử lý một yêu cầu khác.' });
      return false;
    }
    activeRequest = { id: message.requestId, stopRequested: false, recoveryCount: 0 };
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
  return false;
});

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
    await progress({
      requestId: message.requestId,
      stage: 'result',
      status: activeRequest?.stopRequested ? 'stopped' : 'completed',
      conversationId,
      conversationUrl: window.location.href,
      assistantContent: result,
    });
  } catch (error) {
    await progress({
      requestId: message.requestId,
      stage: 'result',
      status: activeRequest?.stopRequested ? 'stopped' : 'failed',
      conversationId,
      conversationUrl: conversationUrl || (started ? window.location.href : undefined),
      assistantContent: started ? latestMessageText('assistant') : undefined,
      errorMessage: errorMessage(error),
    });
  }
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
  return waitFor(() => {
    const match = window.location.pathname.match(/^\/c\/([^/?#]+)/);
    if (!match) return null;
    return { conversationId: decodeURIComponent(match[1]), conversationUrl: window.location.href };
  }, 15_000, 'ChatGPT chưa tạo conversation ID trên URL.');
}

async function waitForAssistant(previousCount, requestId) {
  let baselineCount = previousCount;
  let lastText = '';
  let stableSince = 0;
  let lastActivityAt = Date.now();
  let lastStateCheckAt = 0;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10 * 60_000) {
    const now = Date.now();
    const nodes = assistantNodes();
    const latest = nodes.at(-1);
    const text = latest?.innerText?.trim() || latest?.textContent?.trim() || '';
    const stopButton = findStopButton();
    const threadError = findThreadError();

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
        : idleMs >= SILENT_RECOVERY_GRACE_MS
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
  const candidates = [...document.querySelectorAll([
    'button[data-testid="regenerate-thread-error-button"]',
    '[class*="text-token-text-error"]',
    '[class*="bg-token-surface-error"]',
    '[class*="border-token-surface-error"]',
  ].join(','))].filter(isVisible);
  return candidates.reverse().find((element) => {
    if (!latestUser) return true;
    return Boolean(latestUser.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING);
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

  const reducedMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
  if (!reducedMotion) {
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
  }

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

function normalize(value) { return String(value || '').replace(/\s+/g, ' ').trim().toLowerCase(); }
function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi khi thao tác ChatGPT.'); }
