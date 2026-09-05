// Request-scoped snapshots: event-driven reads, one in-flight write, retry until acknowledged.
(() => {
  const STORAGE_PREFIX = 'chatcmd-think:';
  function restore(requestId) {
    try { return JSON.parse(sessionStorage.getItem(STORAGE_PREFIX + requestId) || 'null'); }
    catch { return null; }
  }
  function create(requestId, submittedContent, { resumed = false, user = null, current = () => true } = {}) {
    const dom = globalThis.ChatCmdTranscript;
    if (!dom || requestId.startsWith('subagent:')) return null;
    const prior = resumed ? restore(requestId) : null;
    const baseline = dom.latestUser()?.id;
    let userId = prior?.userId || user?.id || null;
    let conversationId = prior?.conversationId || (user ? dom.conversationId() : '');
    let messages = Array.isArray(prior?.messages) ? prior.messages : [];
    let revision = Number(prior?.revision) || Date.now();
    let acknowledged = 0;
    let complete = prior?.completed === true;
    let dirty = true;
    let stopped = false;
    let bound = resumed;
    let queued = false;
    let timer = null;
    let inFlight;
    let lastSend = 0;
    const clock = globalThis.ChatCmdCaptureClock;
    const later = (fn, ms) => clock ? clock.later(fn, ms) : setTimeout(fn, ms);
    const cancel = (id) => clock ? clock.cancel(id) : clearTimeout(id);
    const ids = new WeakMap();
    function checkpoint() {
      try { sessionStorage.setItem(STORAGE_PREFIX + requestId, JSON.stringify({ requestId, submittedContent,
        userId, conversationId, messages, revision, completed: complete })); } catch { /* SQLite remains authoritative. */ }
    }
    function scan() {
      if (stopped || !current()) return;
      const id = dom.conversationId();
      if (conversationId && id && id !== conversationId) {
        if (/^WEB:/i.test(conversationId) && userId && dom.latestUser()?.id === userId) { conversationId = id; checkpoint(); }
        else { stop(); return; }
      }
      if (!conversationId && id) conversationId = id;
      const user = dom.latestUser();
      if (!user) return;
      if (!userId) {
        if ((!resumed && user.id === baseline) || user.text !== dom.normalize(submittedContent)) return;
        userId = user.id;
        checkpoint();
      }
      if (user.id !== userId) { stop(); return; }
      const used = new Set();
      let changed = false;
      for (const [index, part] of dom.readParts(user).entries()) {
        let message = ids.get(part.node);
        if (!message) {
          message = messages.find((entry) => !used.has(entry.id) &&
            (entry.content === part.content || part.content.startsWith(entry.content) || entry.content.startsWith(part.content)));
          if (!message && part.messageId) message = messages.find((entry) => entry.id === `${part.messageId}:${index}`);
        }
        if (!message) {
          if (messages.length >= 128) break;
          message = { id: part.messageId ? `${part.messageId}:${index}` : `part-${revision}-${messages.length}`,
            kind: part.kind, content: '' };
          messages.push(message);
        }
        ids.set(part.node, message);
        used.add(message.id);
        if (message.content.startsWith(part.content) && message.content.length > part.content.length) continue;
        const budget = 100_000 - messages.reduce((total, entry) => total + (entry === message ? 0 : [...entry.content].length), 0);
        const content = [...part.content].slice(0, Math.max(0, budget)).join('');
        if (content && (message.content !== content || message.kind !== part.kind)) {
          message.content = content;
          message.kind = part.kind;
          changed = true;
        }
      }
      messages = messages.filter((message) => message.content);
      if (changed) { dirty = true; revision = Math.max(revision + 1, Date.now()); checkpoint(); }
    }
    async function flush(completed = false) {
      scan();
      if (completed && !complete) {
        complete = true; dirty = true; revision = Math.max(revision + 1, Date.now()); checkpoint();
      }
      if (inFlight) {
        const ok = await inFlight;
        if (ok && dirty && !stopped) return flush(completed);
        return !dirty;
      }
      if (!bound || stopped || !current() || !conversationId || !userId) return false;
      if (!dirty) return true;
      const sentRevision = revision;
      const payload = { type: 'chatcmd-chatgpt-progress', stage: 'observation', requestId,
        conversationId, conversationUrl: location.href, userMessageId: userId, revision: sentRevision,
        messages: messages.map((message) => ({ ...message })), completed: complete };
      lastSend = Date.now();
      inFlight = globalThis.ChatCmdRuntime.sendMessage(payload).then((response) => {
        if (!response?.ok || !response.accepted) {
          globalThis.ChatCmdCaptureStatus?.report('error', response?.error || 'Snapshot was not acknowledged');
          return false;
        }
        globalThis.ChatCmdCaptureStatus?.report('synced');
        acknowledged = Math.max(acknowledged, sentRevision);
        dirty = revision > acknowledged;
        return true;
      }).catch((error) => { globalThis.ChatCmdCaptureStatus?.report('error', String(error?.message || error)); return false; });
      try { return await inFlight; } finally { inFlight = null; scheduleSend(); }
    }
    function scheduleSend() {
      if (!bound || !dirty || stopped || !current() || inFlight) return;
      if (timer !== null) cancel(timer);
      // Always retain the trailing update. The last mutation may fall inside the rate limit.
      timer = later(() => { timer = null; void flush(); }, Math.max(25, 500 - (Date.now() - lastSend)));
    }
    function schedule() {
      if (queued || stopped || !current()) return;
      queued = true;
      void Promise.resolve().then(() => {
        queued = false;
        if (stopped || !current()) return;
        scan();
        if (bound && dirty && !inFlight && Date.now() - lastSend >= 500) void flush();
        else scheduleSend();
      });
    }
    const observer = typeof MutationObserver === 'function' ? new MutationObserver(schedule) : null;
    observer?.observe(document.body, { childList: true, characterData: true, subtree: true,
      attributes: true, attributeFilter: ['data-message-id', 'data-turn-id', 'data-turn', 'data-interrupted', 'hidden', 'aria-hidden'] });
    function onPageHide(event) { if (!event.persisted) stop(); }
    window.addEventListener('pagehide', onPageHide);
    function stop() {
      if (stopped) return;
      stopped = true; observer?.disconnect(); if (timer !== null) cancel(timer);
      window.removeEventListener('pagehide', onPageHide);
    }
    function finish() {
      stop();
      if (!dirty) { try { sessionStorage.removeItem(STORAGE_PREFIX + requestId); } catch { /* optional checkpoint */ } }
    }
    return {
      scan, flush, stop, finish,
      bind() { bound = true; return flush(); },
      get answer() { return [...messages].reverse().find((message) => message.kind === 'answer')?.content || ''; },
      get userMessageId() { return userId; },
      get hasTurn() { return Boolean(userId); },
      get active() { return !stopped && current(); },
    };
  }
  globalThis.ChatCmdObserver = Object.freeze({ create, restore });
})();
