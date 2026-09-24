(() => {
  function normalize(value) {
    return String(value || '').replace(/\s+/g, ' ').trim().toLowerCase();
  }

  function isVisible(element) {
    if (!(element instanceof Element)) return false;
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }

  function findVisible(selectors) {
    return findVisibleWithin(document, selectors);
  }

  function findVisibleWithin(root, selectors) {
    for (const selector of selectors) {
      for (const element of root.querySelectorAll(selector)) if (isVisible(element)) return element;
    }
    return null;
  }

  function assistantNodes() {
    return [...document.querySelectorAll('[data-message-author-role="assistant"]')].filter(isVisible);
  }

  function latestMessageText(role) {
    const nodes = [...document.querySelectorAll(`[data-message-author-role="${role}"]`)].filter(isVisible);
    const latest = nodes.at(-1);
    return latest?.innerText?.trim() || latest?.textContent?.trim() || '';
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

  function findStopButton() {
    const composers = [...document.querySelectorAll('form[data-type="unified-composer"]')].filter(isVisible);
    const root = composers.at(-1) || document;
    const direct = findVisibleWithin(root, [
      'button[data-testid="stop-button"]',
      'button[data-testid="stop-generating-button"]',
      'button[aria-label*="Stop" i]',
      'button[aria-label*="Dừng" i]',
      'button[aria-label*="Ngừng" i]',
    ]);
    if (direct) return direct;
    return [...root.querySelectorAll('button')].filter(isVisible).find((button) =>
      /^(stop|dừng|ngừng)(?:\s|$)/i.test(normalize(button.getAttribute('aria-label') || button.textContent))) || null;
  }

  function clickStopButton() {
    const button = findStopButton();
    if (button) button.click();
  }

  function findSendButton(composer) {
    const selectors = [
      'button[data-testid="send-button"]',
      'button[aria-label="Send prompt"]',
      'button[aria-label="Send message"]',
      'button[aria-label="Send"]',
      'button[aria-label="Gửi"]',
      'button[aria-label="Gửi tin nhắn"]',
      'button[title="Send"]',
      'button[title="Gửi"]',
    ];
    const scopes = [];
    const form = composer?.closest?.('form');
    if (form) scopes.push(form);
    let parent = composer?.parentElement;
    for (let depth = 0; parent && depth < 5; depth += 1, parent = parent.parentElement) {
      if (!scopes.includes(parent)) scopes.push(parent);
    }
    for (const scope of scopes) {
      const button = findVisibleWithin(scope, [...selectors, 'button[type="submit"]']);
      if (button) return button;
    }
    return findVisible(selectors);
  }

  globalThis.ChatCmdConversationDom = Object.freeze({
    assistantNodes,
    clickStopButton,
    findSendButton,
    findStopButton,
    findThreadError,
    findVisible,
    isVisible,
    latestMessageText,
    normalize,
  });
})();
