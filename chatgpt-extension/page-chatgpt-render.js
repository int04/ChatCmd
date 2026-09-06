// MAIN world, document_start: retain native animation-frame handles before the page
// caches requestAnimationFrame. Only a leased, hidden chat gets fallback frame ticks.
// No conversation/model data, network hooks, visibility overrides or extension APIs.
(() => {
  if (window.__ChatCmdPageRender?.version === 1) return;
  const nativeRequest = window.requestAnimationFrame;
  const nativeCancel = window.cancelAnimationFrame;
  const pending = new Map();
  const PULSE = 'chatcmd:render-pulse';
  const STATUS = 'chatcmd:render-status';
  let path = location.pathname;
  let leaseUntil = 0;
  let lastTick = -Infinity;
  let fallbackFrames = 0;
  let stopped = false;
  const now = () => performance.now();
  const compatiblePath = (from, to) => from === to
    || ((from === '/' || /\/project$/.test(from) || /\/c\/WEB(?::|%3A)/i.test(from)) && /\/c\//.test(to));
  function announce() {
    document.dispatchEvent(new CustomEvent(STATUS, { detail: JSON.stringify({ version: 1,
      pending: pending.size, fallbackFrames, leased: !stopped && now() < leaseUntil }) }));
  }
  function request(callback) {
    // Preserve native argument validation and native numeric cancellation handles.
    if (typeof callback !== 'function') return nativeRequest.call(window, callback);
    const entry = { callback, path: location.pathname };
    const id = nativeRequest.call(window, (timestamp) => {
      if (!pending.delete(id)) return;
      callback.call(window, timestamp);
    });
    pending.set(id, entry);
    return id;
  }
  function cancel(id) { pending.delete(Number(id)); return nativeCancel.call(window, id); }
  function pulse(event) {
    let message;
    try { message = JSON.parse(event.detail); } catch { return; }
    if (message?.version !== 1 || message.path !== location.pathname || stopped) return;
    const time = now();
    if (path !== location.pathname) {
      if (!compatiblePath(path, location.pathname)) leaseUntil = 0;
      path = location.pathname;
    }
    // This public signal is scheduling-only, never authorization or transcript input.
    if (message.active === true) leaseUntil = time + 2000;
    else leaseUntil = 0;
    if (document.visibilityState !== 'hidden' || time >= leaseUntil || time - lastTick < 100) {
      announce(); return;
    }
    lastTick = time;
    // Snapshot the batch: reentrant rAF requests wait until a later pulse. Cancellation
    // by an earlier callback still wins; errors do not suppress the rest of the batch.
    for (const [id, entry] of [...pending].slice(0, 1000)) {
      if (!pending.has(id) || !compatiblePath(entry.path, path)) continue;
      pending.delete(id);
      nativeCancel.call(window, id);
      try { entry.callback.call(window, time); }
      catch (error) { if (typeof window.reportError === 'function') window.reportError(error); else console.error(error); }
      fallbackFrames += 1;
    }
    announce();
  }
  function suspend() { leaseUntil = 0; }
  function dispose() {
    stopped = true; leaseUntil = 0;
    document.removeEventListener(PULSE, pulse);
    window.removeEventListener('pagehide', suspend);
    window.removeEventListener('popstate', suspend);
    // Queued native callbacks keep their closures and will still run normally.
    if (window.requestAnimationFrame === request) window.requestAnimationFrame = nativeRequest;
    if (window.cancelAnimationFrame === cancel) window.cancelAnimationFrame = nativeCancel;
  }
  window.requestAnimationFrame = request;
  window.cancelAnimationFrame = cancel;
  document.addEventListener(PULSE, pulse);
  window.addEventListener('pagehide', suspend);
  window.addEventListener('popstate', suspend);
  window.__ChatCmdPageRender = Object.freeze({ version: 1, dispose,
    get pending() { return pending.size; }, get fallbackFrames() { return fallbackFrames; } });
})();
