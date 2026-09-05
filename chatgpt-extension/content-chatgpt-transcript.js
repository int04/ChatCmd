// Read only ChatGPT's public rendered transcript. Never inspect private model/reasoning state.
(() => {
  const USERS = '[data-message-author-role="user"],[data-turn="user"]';
  const TURNS = '[data-testid^="conversation-turn"], [data-turn="assistant"]';
  const EXCLUDED = 'button,svg,script,style,.sr-only,.visually-hidden,[hidden],[aria-hidden="true"],[role="alert"],'
    + '[data-chatcmd-ui],.chatcmd-return-button,.chatcmd-approval-panel,.clf-stream,.clf-stage,'
    + '[data-testid*="tool-call"],[data-tool-call-id],span[class*="tool-message"],'
    + '[aria-label="Open tool call list" i]';
  const normalize = (text) => String(text || '').replace(/\s+/g, ' ').trim();
  const conversationId = () => {
    const value = location.pathname.match(/^\/(?:g\/[^/]+\/)?c\/([^/?#]+)(?:\/|$)/)?.[1] || '';
    try { return decodeURIComponent(value); } catch { return value; }
  };
  function plainText(node) {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || '';
    if (!(node instanceof Element) || node.matches(EXCLUDED)) return '';
    const text = [...node.childNodes].map(plainText).join('');
    return /^(P|DIV|SECTION|LI|BR|PRE)$/.test(node.tagName) ? '\n' + text + '\n' : text;
  }
  function userText(node) {
    const body = node.matches('[data-message-author-role="user"]') ? node
      : node.querySelector('[data-message-author-role="user"]') || node;
    const parts = [...body.querySelectorAll('.whitespace-pre-wrap')]
      .filter((part) => !part.parentElement?.closest('.whitespace-pre-wrap'));
    return (parts.length ? parts.map(plainText).join('\n') : plainText(body)).trim();
  }
  function fingerprint(text) {
    let hash = 2166136261;
    for (const char of text) hash = Math.imul(hash ^ char.codePointAt(0), 16777619);
    return (hash >>> 0).toString(16);
  }
  function latestUser() {
    const users = [...document.querySelectorAll(USERS)]
      .filter((node) => !node.parentElement?.closest(USERS));
    const node = users.at(-1);
    if (!node) return null;
    const content = userText(node);
    const id = node.getAttribute('data-message-id') || node.querySelector('[data-message-id]')?.getAttribute('data-message-id')
      || 'dom-user:' + (node.getAttribute('data-turn-id') || node.getAttribute('data-testid') || users.length - 1) + ':' + fingerprint(content);
    return { node, id, text: normalize(content), content };
  }
  function hidden(node) {
    for (let current = node; current && current !== document.body; current = current.parentElement) {
      if (current.matches?.(EXCLUDED)) return true;
      const style = getComputedStyle(current);
      if (style.display === 'none' || style.visibility === 'hidden') return true;
    }
    return false;
  }
  function markdown(node) {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || '';
    if (!(node instanceof Element) || node.matches(EXCLUDED)) return '';
    const tag = node.tagName.toLowerCase();
    if (tag === 'pre') {
      const text = (node.querySelector('code') || node).textContent || '';
      const fence = '`'.repeat(Math.max(3, ...[...text.matchAll(/`+/g)].map((match) => match[0].length + 1)));
      return `\n\n${fence}\n${text.trimEnd()}\n${fence}\n\n`;
    }
    const text = [...node.childNodes].map(markdown).join('');
    if (tag === 'br') return '\n';
    if (tag === 'code') return `\`${text}\``;
    if (tag === 'strong' || tag === 'b') return `**${text}**`;
    if (tag === 'em' || tag === 'i') return `*${text}*`;
    if (/^h[1-6]$/.test(tag)) return `\n\n${'#'.repeat(Number(tag[1]))} ${text}\n\n`;
    if (tag === 'li') return `\n- ${text.trim()}`;
    if (tag === 'blockquote') return `\n\n${text.trim().split('\n').map((line) => `> ${line}`).join('\n')}\n\n`;
    if (tag === 'a') {
      const href = node.getAttribute('href') || '';
      return /^https?:\/\//i.test(href) ? `[${text}](${href.replace(/[()\s]/g, encodeURIComponent)})` : text;
    }
    if (['p', 'div', 'section', 'ul', 'ol', 'table', 'tr'].includes(tag)) return `\n${text}\n`;
    if (tag === 'td' || tag === 'th') return `${text} | `;
    return text;
  }
  function readParts(user) {
    if (!user?.node.isConnected) return [];
    const roots = [...document.querySelectorAll(`${TURNS},[data-message-author-role="assistant"]`)]
      .filter((node) => node.compareDocumentPosition(user.node) & Node.DOCUMENT_POSITION_PRECEDING)
      .filter((node) => !node.matches('[data-turn="user"]') && !node.querySelector(USERS));
    const candidates = new Set();
    for (const root of roots) {
      const blocks = [...root.querySelectorAll('.markdown,[data-interrupted]')];
      if (root.matches('.markdown')) blocks.unshift(root);
      if (!blocks.length && root.matches('[data-message-author-role="assistant"]')) blocks.push(root);
      for (const block of blocks) {
        if (hidden(block)) continue;
        // An expanded tool-result body is not authored assistant prose.
        const tool = block.closest('[data-testid*="tool-call"],[data-tool-call-id],span[class*="tool-message"]');
        if (tool) continue;
        if (block.matches('[data-interrupted]') && block.querySelector('.markdown')) continue;
        if (block.matches('.markdown') && block.parentElement?.closest('.markdown')) continue;
        if (block.matches('[data-interrupted]') && block.parentElement?.closest('[data-interrupted]')) continue;
        candidates.add(block);
      }
    }
    return [...candidates].sort((a, b) => a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1)
      .map((node) => ({ node, kind: node.closest('[data-interrupted]') ? 'commentary' : 'answer',
        content: markdown(node).replace(/\n{3,}/g, '\n\n').trim(),
        messageId: node.closest('[data-message-id]')?.getAttribute('data-message-id') || '' }))
      .filter((part) => part.content);
  }
  globalThis.ChatCmdTranscript = Object.freeze({ latestUser, readParts, normalize, conversationId });
})();
