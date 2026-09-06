// Isolated-world control for the tiny MAIN-world render helper. Only scheduling
// metadata crosses this boundary; the page never supplies a request, URL or transcript.
(() => {
  globalThis.ChatCmdRenderBridge?.stop();
  const owner = globalThis.ChatCmdRuntime.install('render-bridge');
  const clock = globalThis.ChatCmdCaptureClock;
  let stopped = false;
  let bootstrapUntil = 0;
  let bootstrapPath = '';
  let probeJob = null;
  let version = 0;
  let fallbackFrames = 0;
  const current = () => !stopped && globalThis.ChatCmdRuntime.current(owner);
  function pulse(forceIdle = false) {
    if (!current()) return;
    const controller = globalThis.ChatCmdController;
    const request = controller?.current() ? controller.active : null;
    const owned = Boolean(request && !request.resultReported && (!request.observer || request.observer.active));
    const bootstrap = Date.now() < bootstrapUntil && (location.pathname === bootstrapPath
      || ((bootstrapPath === '/' || /\/project$/.test(bootstrapPath)) && /\/c\//.test(location.pathname)));
    const active = !forceIdle && (owned || bootstrap);
    document.dispatchEvent(new CustomEvent('chatcmd:render-pulse', {
      detail: JSON.stringify({ version: 1, path: location.pathname, active }),
    }));
    // Before the first user bubble mounts, the native observer has no request to poll.
    // A short user-gesture lease closes that gap without waking all idle chat tabs.
    if (bootstrap && document.visibilityState === 'hidden' && probeJob === null) {
      probeJob = clock.later(() => { probeJob = null; pulse(); }, 250);
    }
  }
  function status(event) {
    if (!current()) return;
    let value;
    try { value = JSON.parse(event.detail); } catch { return; }
    if (value?.version !== 1 || !Number.isSafeInteger(value.fallbackFrames) || value.fallbackFrames < 0) return;
    version = 1; fallbackFrames = value.fallbackFrames;
    document.documentElement.dataset.chatcmdRenderProtocol = '1';
    document.documentElement.dataset.chatcmdRenderFrames = String(fallbackFrames);
  }
  function submitted(event) {
    if (!event.isTrusted || !current()) return;
    const target = event.target instanceof Element ? event.target : null;
    const enter = event.type === 'keydown' && event.key === 'Enter' && !event.shiftKey && !event.isComposing
      && target?.closest('#prompt-textarea,textarea,[contenteditable="true"]');
    const send = event.type === 'click' && target?.closest('button[data-testid="send-button"],button[aria-label="Send prompt"],button[aria-label="Gửi lời nhắc"]');
    const form = event.type === 'submit' && target?.querySelector('#prompt-textarea,textarea,[contenteditable="true"]');
    if (!enter && !send && !form) return;
    bootstrapUntil = Date.now() + 10000;
    bootstrapPath = location.pathname;
    pulse();
  }
  function visibility() { pulse(); }
  function pageHide() { pulse(true); bootstrapUntil = 0; }
  function stop() {
    if (stopped) return;
    pulse(true); stopped = true;
    if (probeJob !== null) clock.cancel(probeJob);
    document.removeEventListener('chatcmd:render-status', status);
    document.removeEventListener('visibilitychange', visibility);
    for (const name of ['keydown', 'click', 'submit']) document.removeEventListener(name, submitted, true);
    window.removeEventListener('pagehide', pageHide);
  }
  document.addEventListener('chatcmd:render-status', status);
  document.addEventListener('visibilitychange', visibility);
  for (const name of ['keydown', 'click', 'submit']) document.addEventListener(name, submitted, true);
  window.addEventListener('pagehide', pageHide);
  globalThis.ChatCmdRenderBridge = Object.freeze({ pulse, stop, get version() { return version; },
    get fallbackFrames() { return fallbackFrames; } });
  pulse();
})();
