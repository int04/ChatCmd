(() => {
function create(deps) {
  const { findComposer, findSendButton, findStopButton, setComposerText, waitFor, delay } = deps;

  async function attachFiles(composer, rawAttachments) {
    const attachments = normalizeFileAttachments(rawAttachments);
    if (!attachments.length) return;
    const files = attachments.map((attachment) => new File([attachmentFilePart(attachment)], attachment.name, {
      type: attachment.mimeType,
      lastModified: Date.now(),
    }));
    const input = findComposerFileInput(composer);
    if (input) {
      if (!assignFilesToInput(input, files)) throw new Error('ChatGPT không nhận dữ liệu tệp từ bộ chọn tệp.');
      return;
    }

    const baseline = attachmentUiFingerprint(composer);
    const pasteAccepted = pasteFilesIntoComposer(composer, files);
    if (pasteAccepted) return;
    await waitFor(
      () => attachmentUiChanged(baseline, findComposer() || composer, files) ? true : null,
      12_000,
      `ChatGPT không xác nhận tệp đính kèm ${files.map((file) => file.name).join(', ')}.`,
    );
  }

  async function submitPrompt(expectedText) {
    await delay(100);
    let lastWrittenComposer = null;
    let lastWriteAt = 0;
    const button = await waitFor(() => {
      const composer = findComposer();
      if (!composer) return null;
      if (!composerTextMatches(composer, expectedText)) {
        const now = Date.now();
        if (composer !== lastWrittenComposer || now - lastWriteAt >= 360) {
          setComposerText(composer, expectedText);
          lastWrittenComposer = composer;
          lastWriteAt = now;
        }
        return null;
      }
      const candidate = findSendButton();
      if (!candidate || candidate.isConnected === false || candidate.disabled || candidate.getAttribute('aria-disabled') === 'true' || findStopButton()) return null;
      return candidate;
    }, 20_000, 'ChatGPT chưa sẵn sàng gửi: nội dung nhập hoặc tệp đính kèm vẫn đang được đồng bộ.');
    button.click();
    findComposer()?.blur?.();
  }

  return Object.freeze({ attachFiles, submitPrompt });
}

function normalizeFileAttachments(rawAttachments) {
  if (!Array.isArray(rawAttachments)) return [];
  return rawAttachments.flatMap((attachment, index) => {
    if (!attachment || typeof attachment !== 'object' || typeof attachment.content !== 'string' || !attachment.content) return [];
    const rawName = String(attachment.name || `pasted-text-${index + 1}.txt`).split(/[\\/]/).pop().trim();
    if (attachment.encoding === 'base64') {
      return [{
        name: rawName || `attachment-${index + 1}`,
        content: attachment.content,
        mimeType: String(attachment.mimeType || 'application/octet-stream'),
        encoding: 'base64',
      }];
    }
    const name = rawName.toLowerCase().endsWith('.txt') ? rawName : `${rawName || `pasted-text-${index + 1}`}.txt`;
    return [{ name, content: attachment.content, mimeType: 'text/plain;charset=utf-8', encoding: 'utf8' }];
  });
}

function attachmentFilePart(attachment) {
  if (attachment.encoding !== 'base64') return attachment.content;
  const decoded = atob(attachment.content);
  const bytes = new Uint8Array(decoded.length);
  for (let index = 0; index < decoded.length; index += 1) bytes[index] = decoded.charCodeAt(index);
  return bytes;
}

function findComposerFileInput(composer) {
  const form = composer.closest('form');
  const composerScope = composer.closest('[data-type="unified-composer"], [data-testid*="composer" i]');
  const scoped = [
    ...(form ? form.querySelectorAll('input[type="file"]') : []),
    ...(composerScope && composerScope !== form ? composerScope.querySelectorAll('input[type="file"]') : []),
  ].filter((input, index, items) => input instanceof HTMLInputElement && !input.disabled && items.indexOf(input) === index);
  const compatibleScoped = scoped.filter((input) => fileInputScore(input) > 0);
  if (compatibleScoped.length) return compatibleScoped.sort((left, right) => fileInputScore(right) - fileInputScore(left))[0];
  const explicit = [...document.querySelectorAll('input[type="file"][data-testid*="composer" i], input[type="file"][data-testid*="upload" i]')]
    .filter((input) => input instanceof HTMLInputElement && !input.disabled && fileInputScore(input) > 0);
  return explicit.sort((left, right) => fileInputScore(right) - fileInputScore(left))[0] || null;
}

function fileInputScore(input) {
  const accept = String(input.accept || '').toLowerCase();
  if (!accept || accept.includes('*/*')) return 4 + (input.multiple ? 1 : 0);
  if (accept.includes('text') || accept.includes('image') || accept.includes('application') || accept.includes('.')) return 2 + (input.multiple ? 1 : 0);
  return 1;
}

function assignFilesToInput(input, files) {
  const transfer = new DataTransfer();
  for (const existing of input.files || []) transfer.items.add(existing);
  for (const file of files) transfer.items.add(file);
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'files')?.set;
  if (setter) setter.call(input, transfer.files); else input.files = transfer.files;
  const assigned = fileInputContains(input, files);
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
  return assigned;
}

function fileInputContains(input, files) {
  const assigned = Array.from(input.files || []);
  return files.every((file) => assigned.some((candidate) => candidate.name === file.name && candidate.size === file.size && candidate.type === file.type));
}

function pasteFilesIntoComposer(composer, files) {
  const transfer = new DataTransfer();
  for (const file of files) transfer.items.add(file);
  const event = new ClipboardEvent('paste', { bubbles: true, cancelable: true, composed: true, clipboardData: transfer });
  const dispatched = composer.dispatchEvent(event);
  return event.defaultPrevented || dispatched === false;
}

function attachmentUiChanged(baseline, composer, files) {
  const current = attachmentUiFingerprint(composer);
  if (current.count > baseline.count) return true;
  const text = current.text.toLowerCase();
  return files.some((file) => text.includes(file.name.toLowerCase()));
}

function attachmentUiFingerprint(composer) {
  const scope = composer?.closest?.('form') || composer?.closest?.('[data-type="unified-composer"], [data-testid*="composer" i]') || composer?.parentElement || document;
  const selectors = [
    '[data-testid*="attachment" i]', '[data-testid*="file" i]', '[data-testid*="image" i]',
    '[aria-label*="attachment" i]', '[aria-label*="file" i]', '[aria-label*="remove" i] img',
    'img[src^="blob:"]', 'img[src^="data:image/"]',
  ];
  const nodes = new Set();
  for (const selector of selectors) for (const node of scope?.querySelectorAll?.(selector) || []) nodes.add(node);
  return { count: nodes.size, text: String(scope?.textContent || '') };
}

function composerTextMatches(composer, expectedText) {
  return comparableComposerText(readComposerText(composer)) === comparableComposerText(expectedText);
}

function readComposerText(composer) {
  if (!composer) return '';
  if (typeof composer.value === 'string') return composer.value;
  if (typeof composer.innerText === 'string') return composer.innerText;
  return typeof composer.textContent === 'string' ? composer.textContent : '';
}

function comparableComposerText(value) {
  return String(value || '').replace(/\u00a0/g, ' ').replace(/\s+/g, ' ').trim();
}

globalThis.ChatCmdComposerBridge = Object.freeze({ create });
})();
