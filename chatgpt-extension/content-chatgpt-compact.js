// Compact has its own recorder: only the exact marked public answer can become a handoff.
(() => {
  globalThis.ChatCmdCompact?.dispose();
  const protocol = globalThis.ChatCmdCompactProtocol;
  const controller = globalThis.ChatCmdController;
  const dom = globalThis.ChatCmdConversationDom;
  const transcript = globalThis.ChatCmdTranscript;
  const documentToken = crypto.randomUUID();
  const HANDOFF_STABLE_MS = 1200;
  let currentJob = null;
  let ownedConversationId = null;
  let disposed = false;
  let dispatching = false;
  const dispatched = new Set();
  let stableText = '';
  let stableSince = 0;
  let panel;
  let pageUrl = location.href;
  const isCurrent = () => !disposed && controller?.current();
  const setRenderLease = (active) => globalThis.ChatCmdRenderBridge?.setLease?.('compact', active);
  // ProseMirror represents newlines as <p>/<div>/<br>, so textContent alone
  // joins paragraphs and incorrectly rejects the prompt we just inserted.
  function composerText(node) {
    if (!node) return '';
    if (typeof node.value === 'string') return protocol.canonical(node.value);
    function read(part) {
      if (part.nodeType === Node.TEXT_NODE) return part.textContent || '';
      if (!(part instanceof Element)) return '';
      if (part.tagName === 'BR') return '\n';
      const text = [...part.childNodes].map(read).join('');
      return /^(P|DIV|LI)$/.test(part.tagName) ? text + '\n' : text;
    }
    return protocol.canonical(read(node));
  }
  const promptMatches = (node, prompt) => comparable(composerText(node)) === comparable(prompt);
  function readySendButton() {
    const button = dom.findSendButton();
    return button?.isConnected && !button.disabled && button.getAttribute('aria-disabled') !== 'true'
      && !dom.findStopButton() ? button : null;
  }
  const promptsFor = (job, kind) => kind === 'HANDOFF'
    ? [protocol.handoffPrompt(job), protocol.handoffPrompt(job, 1)] : [protocol.resumePrompt(job)];
  const promptFor = (job, kind, composer) => {
    const candidates = promptsFor(job, kind);
    return candidates.find((text) => composer && promptMatches(composer, text)) || candidates[0];
  };
  const comparable = (text) => protocol.canonical(text).replace(/\s+/g, ' ');
  function ownsPage(job, kind) {
    const id = transcript.conversationId();
    return kind === 'HANDOFF' ? id === job.oldConversationId
      : id !== job.oldConversationId && (!job.newConversationId || id === job.newConversationId);
  }
  function markedUser(job, kind) {
    const prefix = protocol.marker(kind, job.id);
    const roots = [...document.querySelectorAll('[data-message-author-role="user"], [data-turn="user"]')]
      .filter((node) => !node.parentElement?.closest('[data-message-author-role="user"], [data-turn="user"]'));
    const read = (node) => transcript.userText(node);
    const matches = roots.filter((node) => protocol.canonical(read(node)).startsWith(prefix));
    if (matches.length !== 1) return { duplicate: matches.length > 1, user: null };
    const node = matches[0];
    if (!promptsFor(job, kind).some((text) => comparable(read(node)) === comparable(text))) return { duplicate: false, user: null };
    const id = node.getAttribute('data-message-id') || node.querySelector('[data-message-id]')?.getAttribute('data-message-id');
    return { duplicate: false, user: id ? { node, id } : null, last: roots.at(-1) === node };
  }
  function show(job) {
    if (!isCurrent()) return;
    if (!job || protocol.terminal(job)) {
      currentJob = null; ownedConversationId = null; setRenderLease(false);
      panel?.remove(); panel = null; return;
    }
    currentJob = job;
    ownedConversationId = transcript.conversationId();
    setRenderLease(true);
    const composer = controller.findComposer();
    const anchor = composer?.closest('form') || composer?.parentElement;
    if (!anchor?.parentElement) return;
    if (!panel?.isConnected) {
      panel = document.createElement('section');
      panel.dataset.chatcmdUi = 'compact';
      panel.className = 'chatcmd-compact-panel';
      panel.setAttribute('role', 'status');
      panel.setAttribute('aria-live', 'polite');
      panel.innerHTML = '<strong>ChatGPT is writing the handoff</strong><ol></ol><p></p>';
      anchor.before(panel);
    }
    if (!document.getElementById('chatcmd-compact-style')) {
      const style = document.createElement('style');
      style.id = 'chatcmd-compact-style';
      style.textContent = `.chatcmd-compact-panel{box-sizing:border-box;width:100%;max-width:900px;margin:12px auto;padding:18px 20px;border:1px solid var(--border-light,#8886);border-radius:16px;background:var(--bg-secondary,var(--main-surface-secondary,#f4f4f4));color:var(--text-primary,#222);font:14px/1.5 system-ui,sans-serif}.chatcmd-compact-panel strong{display:block;font-size:16px;line-height:1.5}.chatcmd-compact-panel ol{display:flex;flex-wrap:wrap;gap:8px 20px;list-style:none;padding:0;margin:14px 0 8px}.chatcmd-compact-panel li{display:flex;gap:7px;align-items:center;opacity:.6}.chatcmd-compact-panel li[aria-current=step]{font-weight:650;opacity:1}.chatcmd-compact-panel li[data-done=true]{opacity:1}.chatcmd-compact-panel p{margin:8px 0 0;overflow-wrap:anywhere;color:inherit}@media(prefers-color-scheme:dark){.chatcmd-compact-panel{background:var(--bg-secondary,#262626);color:var(--text-primary,#eee)}}`;
      document.head.appendChild(style);
    }
    const signature = `${job.phase}\0${job.detail || ''}`;
    if (panel.dataset.signature === signature) return;
    panel.dataset.signature = signature;
    const at = protocol.phases.indexOf(job.phase);
    panel.querySelector('ol').replaceChildren(...protocol.steps.map((text, index) => {
      const item = document.createElement('li');
      item.textContent = `${index < at ? '✓' : index + 1}  ${text}`;
      if (index === at) item.setAttribute('aria-current', 'step');
      item.dataset.done = String(index < at);
      return item;
    }));
    panel.querySelector('p').textContent = job.detail || 'ChatCMD giữ nguyên cuộc trò chuyện và lịch sử. Bạn có thể quay lại sau nếu tab bị đóng.';
  }
  function probe(job, kind) {
    if (!isCurrent() || !ownsPage(job, kind)) throw new Error('Cuộc trò chuyện đã thay đổi; không gửi hoặc thu thập nội dung.');
    show(job);
    const marked = markedUser(job, kind);
    if (marked.duplicate) throw new Error('Có nhiều tin nhắn mang cùng mã compact; cần kiểm tra thủ công, không tự chọn một phản hồi.');
    let text = '';
    if (marked.user && marked.last && kind === 'HANDOFF') {
      const parts = transcript.readParts(marked.user).filter((part) => part.kind === 'answer');
      text = parts.map((part) => part.content).join('\n\n');
    }
    if (text !== stableText || dom.findStopButton()) { stableText = text; stableSince = Date.now(); }
    const handoff = text && Date.now() - stableSince >= HANDOFF_STABLE_MS ? protocol.handoffText(text, job.id) : null;
    const canonicalUrl = new URL(location.href);
    canonicalUrl.hash = ''; canonicalUrl.search = '';
    return { documentToken, conversationId: transcript.conversationId(), conversationUrl: canonicalUrl.href,
      userMessageId: marked.user?.id || null, markerFound: Boolean(marked.user), superseded: Boolean(marked.user && !marked.last),
      generating: Boolean(dom.findStopButton()), handoffText: handoff,
      composerReady: Boolean(controller.findComposer()), draft: Boolean(composerText(controller.findComposer())),
      threadError: Boolean(dom.findThreadError()) };
  }
  async function prepare(job, kind, expectedToken) {
    if (expectedToken !== documentToken || !isCurrent() || !ownsPage(job, kind)) throw new Error('Tab đã được tải lại; đang khôi phục trước khi gửi.');
    show(job);
    if (kind === 'HANDOFF' && dom.findStopButton()) { dom.clickStopButton(); return { ready: false }; }
    if (dom.findStopButton()) return { ready: false };
    await controller.pauseForCompact();
    if (!isCurrent() || !ownsPage(job, kind)) return { ready: false };
    let composer = controller.findComposer();
    if (!composer) return { ready: false };
    const prompt = promptFor(job, kind, composer);
    const text = composerText(composer);
    if (text && !promptMatches(composer, prompt)) throw new Error('Ô nhập đang có bản nháp. Bản nháp được giữ nguyên; hãy lưu hoặc xóa bản nháp để tiếp tục compact.');
    if (kind === 'RESUME' && !text) await controller.selectModel(job.oldModel);
    if (!isCurrent() || !ownsPage(job, kind)) return { ready: false };
    composer = controller.findComposer();
    if (!composer || composerText(composer) !== text) throw new Error('Bản nháp đã thay đổi trong khi chuẩn bị. Nội dung mới của bạn được giữ nguyên.');
    if (!promptMatches(composer, prompt)) {
      controller.setComposerText(composer, prompt);
      globalThis.ChatCmdRenderBridge?.pulse();
    }
    // Let the worker poll while React enables/replaces Send. Rewriting on each poll
    // would restart that update and a page timer may be suspended in a hidden tab.
    return { ready: promptMatches(controller.findComposer(), prompt) && Boolean(readySendButton()), documentToken };
  }
  async function dispatch(job, kind, expectedToken) {
    if (dispatching || expectedToken !== documentToken || !isCurrent() || !ownsPage(job, kind)) throw new Error('Không thể gửi: tài liệu ChatGPT đã thay đổi.');
    const key = `${kind}:${job.id}`;
    const marked = markedUser(job, kind);
    if (marked.duplicate) throw new Error('Có nhiều tin nhắn cùng mã compact; không gửi thêm.');
    if (marked.user || dispatched.has(key)) return { sent: true };
    const composer = controller.findComposer();
    const prompt = promptFor(job, kind, composer);
    if (!composer || !promptMatches(composer, prompt)) throw new Error('Bản nháp đã thay đổi; compact không ghi đè nội dung.');
    const button = readySendButton();
    // This explicit acknowledgement is the ONLY safe retry proof. Exceptions and
    // missing responses remain ambiguous and must keep the durable dispatch fence.
    if (!button) return { sent: false, retryable: true, documentToken };
    dispatching = true;
    try {
      // No await between the last identity/draft check and the irreversible click.
      dispatched.add(key); // Never click twice in this document, even before the marker appears.
      button.click();
      globalThis.ChatCmdRenderBridge?.pulse();
      composer.blur();
      return { sent: true };
    } finally { dispatching = false; }
  }
  // Read-only permission to retire the completed source, never a history tab by URL alone.
  function closeState(job, expectedToken) {
    const marked = markedUser(job, 'HANDOFF');
    const composer = controller.findComposer();
    return { documentToken, conversationId: transcript.conversationId(),
      userMessageId: marked.user?.id || null,
      safeToClose: Boolean(isCurrent() && job.phase === 'completed' && job.handoffText
        && job.newConversationId && job.newConversationId !== job.oldConversationId
        && (!expectedToken || expectedToken === documentToken) && ownsPage(job, 'HANDOFF')
        && marked.user && marked.last && !marked.duplicate && composer
        && !composerText(composer) && !dom.findStopButton()),
    };
  }
  function listener(message, _sender, reply) {
    if (!message?.type?.startsWith('chatcmd-compact-') || !isCurrent()) return false;
    if (message.type === 'chatcmd-compact-clear') {
      if (!message.jobId || !currentJob || message.jobId === currentJob.id) show(null);
      reply({ ok: true }); return false;
    }
    const { job, kind = 'HANDOFF', documentToken: token } = message;
    if (!job?.id) { reply({ ok: false, error: 'Missing compact job.' }); return false; }
    const action = message.type.replace('chatcmd-compact-', '');
    Promise.resolve().then(() => {
      if (action === 'status') {
        if (ownsPage(job, kind)) show(job);
        return { updated: true };
      }
      if (action === 'locate') {
        const found = markedUser(job, kind);
        return { markerFound: Boolean(found.user) && !found.duplicate };
      }
      if (action === 'close-check') return closeState(job, token);
      if (action === 'probe') return probe(job, kind);
      if (action === 'prepare') return prepare(job, kind, token);
      if (action === 'dispatch') return dispatch(job, kind, token);
      throw new Error('Unsupported compact action.');
    }).then((value) => reply({ ok: true, ...value })).catch((error) => reply({ ok: false, error: String(error?.message || error) }));
    return true;
  }
  chrome.runtime.onMessage.addListener(listener);
  function dispose() {
    currentJob = null; ownedConversationId = null; setRenderLease(false);
    disposed = true; panel?.remove(); chrome.runtime.onMessage.removeListener(listener);
    window.removeEventListener('pageshow', wake); window.removeEventListener('popstate', wake);
  }
  globalThis.ChatCmdCompact = Object.freeze({
    version: 3,
    get busy() { return Boolean(currentJob && !protocol.terminal(currentJob) && ownedConversationId === transcript.conversationId()); },
    dispose, probe,
  });
  // Reconcile with the durable worker after reload, BFCache restore and SPA navigation.
  function wake() {
    if (!isCurrent()) return;
    if (pageUrl !== location.href) {
      pageUrl = location.href; currentJob = null; ownedConversationId = null; setRenderLease(false);
      panel?.remove(); panel = null;
    }
    void globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-compact-wake' }).catch(() => {});
  }
  window.addEventListener('pageshow', wake);
  window.addEventListener('popstate', wake);
  wake();
})();
