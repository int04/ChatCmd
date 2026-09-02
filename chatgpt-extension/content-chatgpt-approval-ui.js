(() => {
  if (globalThis.ChatCmdGlobalApprovalUi) return;

  const ROOT_ID = 'chatcmd-global-approval-root';
  let items = [];
  let busy = false;
  let error = '';
  let rejecting = false;
  let reason = '';
  let lastSoundKey = null;
  let previousTitle = null;
  let countdownTimer = null;

  function ensureRoot() {
    let host = document.getElementById(ROOT_ID);
    if (host) return host.shadowRoot;
    host = document.createElement('div');
    host.id = ROOT_ID;
    Object.assign(host.style, { position: 'fixed', inset: '0', zIndex: '2147483647', pointerEvents: 'none' });
    const shadow = host.attachShadow({ mode: 'open' });
    document.documentElement.appendChild(host);
    return shadow;
  }

  function render(nextItems) {
    items = Array.isArray(nextItems) ? nextItems : [];
    const current = items[0];
    const shadow = ensureRoot();
    if (countdownTimer) clearInterval(countdownTimer);
    countdownTimer = null;

    if (!current) {
      shadow.replaceChildren();
      busy = false;
      error = '';
      rejecting = false;
      reason = '';
      lastSoundKey = null;
      if (previousTitle !== null) {
        document.title = previousTitle;
        previousTitle = null;
      }
      return;
    }

    if (previousTitle === null) previousTitle = document.title;
    document.title = 'Xin phê duyệt · ChatCMD';
    if (lastSoundKey !== current.key) {
      lastSoundKey = current.key;
      playApprovalSound();
    }

    shadow.innerHTML = `
      <style>
        :host { all: initial; }
        * { box-sizing: border-box; }
        .backdrop { position: fixed; inset: 0; display: grid; place-items: center; padding: 24px; background: rgba(2,6,23,.56); backdrop-filter: blur(7px); -webkit-backdrop-filter: blur(7px); pointer-events: auto; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #f8fafc; }
        .card { width: min(620px, calc(100vw - 32px)); max-height: min(760px, calc(100vh - 48px)); overflow: auto; border: 1px solid rgba(148,163,184,.2); border-radius: 24px; background: linear-gradient(155deg, rgba(15,23,42,.99), rgba(17,24,39,.99)); box-shadow: 0 28px 90px rgba(2,6,23,.58), 0 0 0 1px rgba(255,255,255,.04) inset; }
        .header { display: flex; gap: 14px; align-items: flex-start; padding: 22px 22px 17px; border-bottom: 1px solid rgba(148,163,184,.14); }
        .icon { flex: 0 0 46px; height: 46px; display: grid; place-items: center; border-radius: 15px; background: rgba(245,158,11,.13); color: #fbbf24; box-shadow: 0 0 0 1px rgba(251,191,36,.16) inset; }
        .copy { min-width: 0; flex: 1; }
        .eyebrow { color: #94a3b8; font-size: 11px; line-height: 1.2; font-weight: 800; letter-spacing: .12em; text-transform: uppercase; }
        h2 { margin: 5px 0 4px; font-size: 19px; line-height: 1.25; letter-spacing: -.02em; }
        .desc { margin: 0; color: #cbd5e1; font-size: 13px; line-height: 1.55; }
        .count { flex: 0 0 auto; padding: 6px 9px; border-radius: 999px; background: rgba(59,130,246,.12); border: 1px solid rgba(96,165,250,.2); color: #bfdbfe; font-size: 11px; font-weight: 800; }
        .body { display: grid; gap: 14px; padding: 18px 22px 6px; }
        .meta { display: grid; grid-template-columns: 1fr auto; gap: 10px; align-items: center; padding: 12px 14px; border-radius: 15px; background: rgba(30,41,59,.62); border: 1px solid rgba(148,163,184,.12); }
        .meta strong { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
        .meta small { display: block; margin-top: 4px; color: #94a3b8; font-size: 11px; }
        .timer { color: #fde68a; font-size: 12px; font-weight: 800; font-variant-numeric: tabular-nums; }
        .tool { display: flex; align-items: center; gap: 8px; font-size: 13px; color: #e2e8f0; }
        .tool code { padding: 4px 8px; border-radius: 8px; background: rgba(15,23,42,.75); color: #c4b5fd; font: 700 12px/1.3 ui-monospace, SFMono-Regular, Menlo, monospace; }
        pre { max-height: 220px; overflow: auto; margin: 0; padding: 13px 14px; border-radius: 14px; background: #020617; border: 1px solid rgba(148,163,184,.13); color: #cbd5e1; white-space: pre-wrap; overflow-wrap: anywhere; font: 500 11px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace; }
        .reject { display: grid; gap: 8px; }
        label { color: #cbd5e1; font-size: 12px; font-weight: 700; }
        textarea { width: 100%; min-height: 82px; resize: vertical; border: 1px solid rgba(148,163,184,.22); border-radius: 12px; padding: 10px 12px; outline: none; background: rgba(2,6,23,.72); color: #f8fafc; font: 500 13px/1.45 inherit; }
        textarea:focus { border-color: rgba(96,165,250,.72); box-shadow: 0 0 0 3px rgba(59,130,246,.14); }
        .error { margin: 0; padding: 10px 12px; border-radius: 12px; border: 1px solid rgba(248,113,113,.25); background: rgba(127,29,29,.22); color: #fecaca; font-size: 12px; line-height: 1.45; }
        .actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 9px; padding: 17px 22px 22px; }
        button { min-height: 40px; border: 0; border-radius: 12px; padding: 0 15px; font: 800 12px/1 system-ui, sans-serif; cursor: pointer; transition: transform .15s ease, filter .15s ease, opacity .15s ease; }
        button:hover:not(:disabled) { transform: translateY(-1px); filter: brightness(1.08); }
        button:disabled { opacity: .5; cursor: wait; }
        .secondary { background: #1e293b; color: #e2e8f0; box-shadow: 0 0 0 1px rgba(148,163,184,.18) inset; }
        .danger { background: #7f1d1d; color: #fee2e2; }
        .similar { background: #4c1d95; color: #ede9fe; }
        .primary { background: #2563eb; color: #eff6ff; }
        @media (max-width: 640px) { .backdrop { padding: 12px; } .card { width: 100%; border-radius: 18px; } .header, .body, .actions { padding-left: 16px; padding-right: 16px; } .count { display: none; } }
        @media (prefers-reduced-motion: no-preference) { .card { animation: pop .22s cubic-bezier(.16,1,.3,1); } @keyframes pop { from { opacity: 0; transform: translateY(10px) scale(.985); } } }
      </style>
      <div class="backdrop" role="alertdialog" aria-modal="true" aria-labelledby="chatcmd-approval-title" aria-describedby="chatcmd-approval-description">
        <section class="card">
          <header class="header">
            <div class="icon" aria-hidden="true">${current.kind === 'conversation' ? '✦' : '›_'}</div>
            <div class="copy"><div class="eyebrow">ChatCMD · Global approval</div><h2 id="chatcmd-approval-title">Xin phê duyệt</h2><p class="desc" id="chatcmd-approval-description">${escapeHtml(current.kind === 'conversation' ? 'Một đoạn trò chuyện mới đang chờ quyền chạy lệnh.' : 'Agent đang chờ quyền thực thi một thao tác trên máy của bạn.')}</p></div>
            <span class="count">${items.length} đang chờ</span>
          </header>
          <div class="body">
            <div class="meta"><div><strong>${escapeHtml(current.title || current.tool || current.taskId)}</strong><small>Task #${escapeHtml(shortId(current.taskId))}</small></div><span class="timer" data-timer></span></div>
            ${current.kind === 'activity' ? `<div class="tool">Tool <code>${escapeHtml(current.tool || 'tool')}</code></div><pre>${escapeHtml(formatInput(current.input))}</pre>` : ''}
            ${rejecting ? `<div class="reject"><label for="chatcmd-reject-reason">Lý do từ chối (không bắt buộc)</label><textarea id="chatcmd-reject-reason" maxlength="2000" placeholder="Ví dụ: Không chạy lệnh này, hãy dùng cách chỉ đọc.">${escapeHtml(reason)}</textarea></div>` : ''}
            ${error ? `<p class="error" role="alert">${escapeHtml(error)}</p>` : ''}
          </div>
          <footer class="actions">
            ${rejecting ? '<button class="secondary" data-action="cancel-reject">Quay lại</button><button class="danger" data-action="confirm-reject">Xác nhận từ chối</button>' : `<button class="danger" data-action="reject">Từ chối</button>${current.kind === 'activity' ? '<button class="similar" data-action="allowSimilar">Cho phép tương tự</button>' : ''}<button class="primary" data-action="allow">Phê duyệt</button>`}
          </footer>
        </section>
      </div>
    `;

    const textarea = shadow.querySelector('textarea');
    if (textarea) {
      textarea.addEventListener('input', (event) => { reason = event.target.value; });
      queueMicrotask(() => textarea.focus());
    }
    for (const button of shadow.querySelectorAll('button[data-action]')) {
      button.disabled = busy;
      button.addEventListener('click', () => void handleAction(button.dataset.action));
    }
    updateTimer(shadow, current);
    countdownTimer = setInterval(() => updateTimer(shadow, current), 250);
  }

  async function handleAction(action) {
    const current = items[0];
    if (!current || busy) return;
    if (action === 'reject') {
      rejecting = true;
      error = '';
      render(items);
      return;
    }
    if (action === 'cancel-reject') {
      rejecting = false;
      reason = '';
      error = '';
      render(items);
      return;
    }
    const decision = action === 'confirm-reject' ? 'reject' : action;
    busy = true;
    error = '';
    render(items);
    try {
      const response = await chrome.runtime.sendMessage({ type: 'chatcmd-approval-decision', item: current, decision, reason: decision === 'reject' ? reason : undefined });
      if (!response?.ok) throw new Error(response?.error || 'Không thể gửi quyết định phê duyệt tới ChatCMD.');
      items = items.filter((item) => item.key !== current.key);
      rejecting = false;
      reason = '';
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure || 'Không thể gửi quyết định phê duyệt.');
    } finally {
      busy = false;
      render(items);
    }
  }

  function updateTimer(shadow, current) {
    const target = shadow.querySelector('[data-timer]');
    if (!target) return;
    const deadline = Date.parse(current.deadlineUtc || '');
    if (!Number.isFinite(deadline)) { target.textContent = 'Đang chờ'; return; }
    const remaining = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
    target.textContent = `${remaining}s`;
  }

  function playApprovalSound() {
    try {
      const AudioContextCtor = globalThis.AudioContext || globalThis.webkitAudioContext;
      if (!AudioContextCtor) return;
      const context = new AudioContextCtor();
      const now = context.currentTime;
      for (const [offset, frequency] of [[0, 660], [.13, 880]]) {
        const oscillator = context.createOscillator();
        const gain = context.createGain();
        oscillator.frequency.value = frequency;
        gain.gain.setValueAtTime(0.0001, now + offset);
        gain.gain.exponentialRampToValueAtTime(0.08, now + offset + .015);
        gain.gain.exponentialRampToValueAtTime(0.0001, now + offset + .11);
        oscillator.connect(gain).connect(context.destination);
        oscillator.start(now + offset);
        oscillator.stop(now + offset + .12);
      }
      setTimeout(() => void context.close(), 500);
    } catch { /* Browser autoplay policy can block audio until user interaction. */ }
  }

  function formatInput(value) {
    if (value == null) return 'Không có dữ liệu đầu vào.';
    try {
      const text = typeof value === 'string' ? value : JSON.stringify(value, null, 2);
      return text.length > 4000 ? `${text.slice(0, 4000)}\n…` : text;
    } catch { return String(value); }
  }
  function shortId(value) { const text = String(value || ''); return text.length > 18 ? `${text.slice(0, 8)}…${text.slice(-7)}` : text; }
  function escapeHtml(value) { return String(value ?? '').replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]); }

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (message?.type !== 'chatcmd-global-approval-state') return false;
    render(message.items);
    sendResponse({ ok: true });
    return false;
  });

  void chrome.runtime.sendMessage({ type: 'chatcmd-approval-state-request' }, (response) => {
    if (response?.ok) render(response.items);
  });

  globalThis.ChatCmdGlobalApprovalUi = Object.freeze({ render });
})();
