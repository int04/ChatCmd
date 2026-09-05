// Native ChatGPT sends and ChatCMD sends share one request-scoped recorder.
// Enrollment observes public messages only; it never presses Send or calls MCP.
(() => {
  globalThis.ChatCmdNativeCapture?.stop();
  const controller = globalThis.ChatCmdController;
  const dom = globalThis.ChatCmdTranscript;
  const clock = globalThis.ChatCmdCaptureClock;
  const later = (fn, ms, options) => clock ? clock.later(fn, ms, options) : setTimeout(fn, ms);
  const cancel = (id) => clock ? clock.cancel(id) : clearTimeout(id);
  const seen = new Set();
  let stopped = false;
  let pending = false;
  let retryAt = 0;
  let queued = false;
  let pollTimer;
  let retryTimer;
  let lastDiagnostic = '';
  function report(state, detail = '') {
    if (!controller?.current()) return;
    document.documentElement.dataset.chatcmdCaptureState = state;
    document.documentElement.dataset.chatcmdCaptureError = detail;
    const diagnostic = `${state}:${detail}`;
    if (diagnostic === lastDiagnostic) return;
    lastDiagnostic = diagnostic;
    if (detail) console.warn('[ChatCMD capture]', detail);
    void globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-capture-diagnostic', state, detail }).catch(() => {});
  }
  globalThis.ChatCmdCaptureStatus = Object.freeze({ report });
  const keyFor = (user) => user && dom.conversationId() ? `${dom.conversationId()}\0${user.id}` : '';
  function remember(key) { seen.add(key); if (seen.size > 128) seen.delete(seen.values().next().value); }
  function retry(ms) {
    cancel(retryTimer);
    retryTimer = later(() => void tick(), ms);
  }
  async function tick() {
    if (stopped || !controller?.current()) { stop(); return; }
    const user = dom.latestUser();
    const key = keyFor(user);
    if (!key || !user.content.trim()) return;
    if (controller.active) {
      if (controller.active.observer?.userMessageId === user.id && controller.active.observer.active) remember(key);
      else if (controller.active.observer?.userMessageId) retry(500);
      return;
    }
    if (seen.has(key) || pending || Date.now() < retryAt) return;
    pending = true;
    try {
      const response = await globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-chatgpt-native-turn',
        conversationId: dom.conversationId(), conversationUrl: location.href,
        userMessageId: user.id, content: user.content });
      if (stopped || !controller.current() || keyFor(dom.latestUser()) !== key || controller.active) return;
      if (!response?.ok || !response.request) throw new Error(response?.error || 'ChatCMD did not acknowledge the browser turn.');
      report('recording');
      remember(key);
      if (response.request.status === 'completed' && response.request.hasFinalResponse) return;
      void controller.adopt(response.request, user).then(() => {
        if (stopped || !controller.current()) return;
        if (keyFor(dom.latestUser()) === key && document.documentElement.dataset.chatcmdCaptureState === 'error') {
          seen.delete(key); retryAt = Date.now() + 5000; retry(5000);
        }
        schedule();
      });
    } catch (error) {
      retryAt = Date.now() + 5000;
      report('error', String(error?.message || error));
      if (!stopped) retry(5000);
    } finally { pending = false; }
  }
  function schedule() {
    if (queued || stopped) return;
    queued = true;
    void Promise.resolve().then(() => { queued = false; if (!stopped) void tick(); });
  }
  const observer = new MutationObserver(schedule);
  observer.observe(document.body, { subtree: true, childList: true, characterData: true });
  function poll() {
    if (stopped) return;
    void tick();
    // Idle polling is only a local fallback, never a perpetual service-worker keepalive.
    pollTimer = later(poll, 750, { background: false });
  }
  function stop() {
    if (stopped) return;
    stopped = true; observer.disconnect(); cancel(pollTimer); cancel(retryTimer);
    document.removeEventListener('visibilitychange', schedule);
    window.removeEventListener('pageshow', schedule);
  }
  document.addEventListener('visibilitychange', schedule);
  window.addEventListener('pageshow', schedule);
  pollTimer = later(poll, 750, { background: false });
  globalThis.ChatCmdNativeCapture = Object.freeze({ stop, tick, schedule });
  Promise.resolve(globalThis.ChatCmdResumeReady).then(() => void tick());
})();
