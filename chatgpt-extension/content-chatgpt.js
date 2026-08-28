let activeRequest = null;

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'chatcmd-chatgpt-run') {
    if (activeRequest) {
      sendResponse({ ok: false, error: 'Tab ChatGPT này đang xử lý một yêu cầu khác.' });
      return false;
    }
    activeRequest = { id: message.requestId, stopRequested: false };
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
    const result = await waitForAssistant(assistantCount);
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
  const button = findVisible([
    'button[data-testid="model-switcher-dropdown-button"]',
    'button[aria-label*="model" i]',
    'button[id*="model" i]',
  ]);
  if (!button) throw new Error(`Không tìm thấy bộ chọn model ChatGPT. Chọn Auto hoặc mở đúng giao diện chatgpt.com.`);
  button.click();
  await delay(250);
  const option = await waitFor(() => {
    const candidates = [...document.querySelectorAll('[role="menuitem"], [role="option"], [data-radix-collection-item]')].filter(isVisible);
    const wanted = normalize(target);
    return candidates.find((item) => normalize(item.textContent).includes(wanted)) || null;
  }, 4_000, `Không tìm thấy model “${target}” trong menu ChatGPT.`);
  option.click();
  await delay(200);
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

async function waitForAssistant(previousCount) {
  let lastText = '';
  let stableSince = 0;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10 * 60_000) {
    const nodes = assistantNodes();
    const latest = nodes.at(-1);
    const text = latest?.innerText?.trim() || latest?.textContent?.trim() || '';
    if (activeRequest?.stopRequested) {
      clickStopButton();
      if (!findStopButton()) return text;
    }
    if (nodes.length > previousCount && text) {
      if (text === lastText) {
        if (!stableSince) stableSince = Date.now();
        if (!findStopButton() && Date.now() - stableSince > 1_200) return text;
      } else {
        lastText = text;
        stableSince = Date.now();
      }
    }
    await delay(350);
  }
  throw new Error('Quá lâu chưa nhận được phản hồi hoàn tất từ ChatGPT.');
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

function normalize(value) { return String(value || '').replace(/\s+/g, ' ').trim().toLowerCase(); }
function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function errorMessage(error) { return error instanceof Error ? error.message : String(error || 'Lỗi khi thao tác ChatGPT.'); }
