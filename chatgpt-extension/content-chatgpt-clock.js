// Worker-driven capture scheduling also pulses the separate MAIN-world frame bridge.
// Worker replies wake due capture jobs when the document's own timers are delayed.
(() => {
  globalThis.ChatCmdCaptureClock?.stop();
  const owner = globalThis.ChatCmdRuntime.install('capture-clock');
  const jobs = new Map();
  let sequence = 0;
  let timer = null;
  let remote = false;
  let retryAt = 0;
  let stopped = false;
  const current = () => !stopped && globalThis.ChatCmdRuntime.current(owner);
  const channel = typeof MessageChannel === 'function' ? new MessageChannel() : null;
  if (channel) channel.port1.onmessage = () => drain();

  function later(fn, ms, { background = true } = {}) {
    if (!current()) return null;
    const id = ++sequence;
    jobs.set(id, { fn, due: Date.now() + Math.max(0, Number(ms) || 0), background });
    plan();
    return id;
  }
  function cancel(id) { jobs.delete(id); plan(); }
  function plan() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    if (!current() || !jobs.size) return;
    const next = Math.min(...[...jobs.values()].map((job) => job.due));
    timer = setTimeout(() => {
      timer = null;
      // Run the continuation in a message task, outside the local timer chain.
      if (channel) channel.port2.postMessage(0); else drain();
    }, Math.max(0, next - Date.now()));
    requestWake();
  }
  function requestWake() {
    if (remote || !current() || document.visibilityState !== 'hidden' || Date.now() < retryAt) return;
    const deadlines = [...jobs.values()].filter((job) => job.background).map((job) => job.due);
    if (!deadlines.length) return;
    remote = true;
    const delayMs = Math.min(1000, Math.max(25, Math.min(...deadlines) - Date.now()));
    void globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-capture-wake', delayMs }).then((reply) => {
      if (!reply?.ok || reply.clockProtocol !== 1) throw new Error(reply?.error || 'Background capture clock unavailable');
      remote = false;
      retryAt = 0;
      if (current()) drain();
    }).catch((error) => {
      remote = false;
      retryAt = Date.now() + 2000;
      if (current()) {
        globalThis.ChatCmdCaptureStatus?.report('error', String(error?.message || error));
        plan();
      }
    });
  }
  function drain() {
    if (!current()) { stop(); return; }
    globalThis.ChatCmdRenderBridge?.pulse();
    const now = Date.now();
    for (const [id, job] of [...jobs]) {
      if (job.due > now || !jobs.has(id)) continue;
      jobs.delete(id);
      try { job.fn(); } catch (error) { console.error('[ChatCMD capture clock]', error); }
    }
    plan();
  }
  function onVisibility() { if (current()) drain(); }
  function onPageHide(event) { if (!event.persisted) stop(); }
  function stop() {
    globalThis.ChatCmdRenderBridge?.pulse(true);
    stopped = true;
    clearTimeout(timer);
    jobs.clear();
    channel?.port1.close();
    channel?.port2.close();
    document.removeEventListener('visibilitychange', onVisibility);
    document.removeEventListener('resume', onVisibility);
    window.removeEventListener('pagehide', onPageHide);
  }
  document.addEventListener('visibilitychange', onVisibility);
  document.addEventListener('resume', onVisibility);
  window.addEventListener('pagehide', onPageHide);
  globalThis.ChatCmdCaptureClock = Object.freeze({ later, cancel, wake: drain, stop,
    sleep: (ms) => new Promise((resolve) => later(resolve, ms)), version: 1 });
})();
