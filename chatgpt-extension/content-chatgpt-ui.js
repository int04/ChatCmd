(() => {
  function renderReturnToChatCmd(enabled) {
    renderChatCmdControlledState(enabled);
    const id = 'chatcmd-return-to-app';
    document.getElementById(id)?.remove();
    if (!enabled) return;

    const button = document.createElement('button');
    button.id = id;
    button.type = 'button';
    button.setAttribute('aria-label', 'Quay lại ChatCMD');
    button.title = 'Quay lại ChatCMD';
    button.innerHTML = `
      <span data-chatcmd-return-icon aria-hidden="true">↩</span>
      <span data-chatcmd-return-copy>
        <strong>Quay lại ChatCMD</strong>
        <small>Bấm để trở về</small>
      </span>
      <i data-chatcmd-return-dot aria-hidden="true"></i>
    `;

    Object.assign(button.style, {
      position: 'fixed', right: '24px', bottom: '112px', zIndex: '2147483647',
      display: 'grid', gridTemplateColumns: '46px minmax(0,1fr) 10px', alignItems: 'center', gap: '11px',
      minWidth: '220px', minHeight: '66px', padding: '9px 15px 9px 10px',
      border: '1px solid rgba(255,255,255,.28)', borderRadius: '20px',
      background: 'linear-gradient(135deg,rgba(124,58,237,.98),rgba(37,99,235,.98))', color: '#fff',
      boxShadow: '0 18px 46px rgba(76,29,149,.42),0 0 0 1px rgba(255,255,255,.08) inset',
      backdropFilter: 'blur(18px)', WebkitBackdropFilter: 'blur(18px)',
      font: '600 13px/1.2 system-ui,-apple-system,Segoe UI,sans-serif', cursor: 'pointer',
      transformOrigin: 'right center', transition: 'transform 180ms ease,box-shadow 180ms ease,filter 180ms ease',
      isolation: 'isolate', overflow: 'hidden'
    });

    const icon = button.querySelector('[data-chatcmd-return-icon]');
    const copy = button.querySelector('[data-chatcmd-return-copy]');
    const dot = button.querySelector('[data-chatcmd-return-dot]');
    Object.assign(icon.style, {
      width: '46px', height: '46px', display: 'grid', placeItems: 'center', borderRadius: '15px',
      background: 'rgba(255,255,255,.16)', boxShadow: '0 0 0 1px rgba(255,255,255,.14) inset',
      fontSize: '25px', fontWeight: '800'
    });
    Object.assign(copy.style, { minWidth: '0', display: 'grid', gap: '4px', textAlign: 'left' });
    Object.assign(copy.querySelector('strong').style, { fontSize: '14px', letterSpacing: '-.01em', whiteSpace: 'nowrap' });
    Object.assign(copy.querySelector('small').style, { color: 'rgba(255,255,255,.78)', fontSize: '11px', fontWeight: '550' });
    Object.assign(dot.style, {
      width: '9px', height: '9px', borderRadius: '50%', background: '#86efac',
      boxShadow: '0 0 0 4px rgba(134,239,172,.16),0 0 16px rgba(134,239,172,.8)'
    });

    button.animate([
      { boxShadow: '0 18px 46px rgba(76,29,149,.42),0 0 0 0 rgba(139,92,246,.34)' },
      { boxShadow: '0 20px 54px rgba(76,29,149,.52),0 0 0 12px rgba(139,92,246,0)' }
    ], { duration: 1900, iterations: Infinity, easing: 'ease-out' });
    icon.animate([
      { transform: 'translateX(0) rotate(0deg)' },
      { transform: 'translateX(-3px) rotate(-8deg)' },
      { transform: 'translateX(0) rotate(0deg)' }
    ], { duration: 1450, iterations: Infinity, easing: 'ease-in-out' });
    dot.animate([{ opacity: .55 }, { opacity: 1 }, { opacity: .55 }], { duration: 1100, iterations: Infinity, easing: 'ease-in-out' });
    button.animate([
      { opacity: 0, transform: 'translateX(26px) scale(.9)' },
      { opacity: 1, transform: 'translateX(0) scale(1.04)' },
      { opacity: 1, transform: 'translateX(0) scale(1)' }
    ], { duration: 520, easing: 'cubic-bezier(.16,1,.3,1)' });

    button.addEventListener('mouseenter', () => {
      button.style.transform = 'translateY(-4px) scale(1.035)';
      button.style.filter = 'brightness(1.1) saturate(1.08)';
      button.style.boxShadow = '0 24px 64px rgba(76,29,149,.55),0 0 0 1px rgba(255,255,255,.18) inset';
    });
    button.addEventListener('mouseleave', () => {
      button.style.transform = '';
      button.style.filter = '';
      button.style.boxShadow = '0 18px 46px rgba(76,29,149,.42),0 0 0 1px rgba(255,255,255,.08) inset';
    });
    button.addEventListener('click', () => {
      button.disabled = true;
      button.style.opacity = '.7';
      copy.querySelector('small').textContent = 'Đang quay lại…';
      chrome.runtime.sendMessage({ type: 'chatcmd-return-to-source' }, () => {
        button.disabled = false;
        button.style.opacity = '';
        copy.querySelector('small').textContent = 'Bấm để trở về';
      });
    });

    document.documentElement.appendChild(button);
  }

  function renderChatCmdControlledState(enabled) {
    const bannerId = 'chatcmd-controlled-banner';
    const frameId = 'chatcmd-controlled-frame';
    document.getElementById(bannerId)?.remove();
    document.getElementById(frameId)?.remove();
    if (!enabled) return;

    const banner = document.createElement('section');
    banner.id = bannerId;
    banner.setAttribute('role', 'status');
    banner.setAttribute('aria-live', 'polite');
    banner.innerHTML = `
      <span data-chatcmd-warning-icon aria-hidden="true">
        <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.3 2.9 1.8 17.1A2 2 0 0 0 3.5 20h17a2 2 0 0 0 1.7-2.9L13.7 2.9a2 2 0 0 0-3.4 0Z"/>
          <path d="M12 9v4"/><path d="M12 17h.01"/>
        </svg>
      </span>
      <span data-chatcmd-warning-copy>
        <strong>Đừng thao tác với tab ChatGPT này</strong>
        <span>Tab ChatGPT này đang được xử lý bởi ChatCMD, xin đừng thao tác trên tab trình duyệt này khi bạn vẫn đang sử dụng trên ChatCMD vì có thể gây lỗi cho bên ChatCMD. Chỉ đóng tab này nếu như bạn không còn sử dụng bên ChatCMD nữa.</span>
      </span>
      <span data-chatcmd-warning-state><i></i> ChatCMD đang sử dụng</span>
    `;
    Object.assign(banner.style, {
      position: 'fixed', left: '24px', top: '24px', zIndex: '2147483646',
      width: 'min(720px,calc(100vw - 48px))', boxSizing: 'border-box', display: 'grid',
      gridTemplateColumns: '52px minmax(0,1fr)', gap: '14px', alignItems: 'start', padding: '16px 18px',
      color: '#fff', border: '1px solid rgba(253,224,71,.54)', borderRadius: '22px',
      background: 'linear-gradient(135deg,rgba(15,23,42,.96),rgba(88,28,135,.94) 54%,rgba(124,45,18,.94))',
      boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 34px rgba(250,204,21,.18)',
      backdropFilter: 'blur(20px) saturate(1.25)', WebkitBackdropFilter: 'blur(20px) saturate(1.25)',
      font: '500 14px/1.55 system-ui,-apple-system,Segoe UI,sans-serif', pointerEvents: 'none', isolation: 'isolate', overflow: 'hidden'
    });
    const icon = banner.querySelector('[data-chatcmd-warning-icon]');
    const copy = banner.querySelector('[data-chatcmd-warning-copy]');
    const title = copy.querySelector('strong');
    const body = copy.querySelector('span');
    const state = banner.querySelector('[data-chatcmd-warning-state]');
    const dot = state.querySelector('i');
    Object.assign(icon.style, {
      width: '52px', height: '52px', display: 'grid', placeItems: 'center', borderRadius: '17px',
      color: '#fde68a', background: 'linear-gradient(135deg,rgba(245,158,11,.26),rgba(234,88,12,.2))',
      boxShadow: '0 0 0 1px rgba(253,224,71,.24) inset,0 0 24px rgba(245,158,11,.18)'
    });
    Object.assign(copy.style, { display: 'grid', gap: '6px', minWidth: '0', paddingRight: '4px' });
    Object.assign(title.style, { fontSize: '17px', lineHeight: '1.3', letterSpacing: '-.02em', fontWeight: '800', color: '#fff7ed' });
    Object.assign(body.style, { color: 'rgba(255,255,255,.8)', fontSize: '13px', lineHeight: '1.55' });
    Object.assign(state.style, {
      gridColumn: '2', justifySelf: 'start', display: 'inline-flex', alignItems: 'center', gap: '7px',
      marginTop: '2px', padding: '5px 9px', borderRadius: '999px', color: '#fef3c7',
      background: 'rgba(245,158,11,.12)', border: '1px solid rgba(253,224,71,.2)', fontSize: '11px', fontWeight: '700'
    });
    Object.assign(dot.style, { width: '7px', height: '7px', borderRadius: '50%', background: '#facc15', boxShadow: '0 0 0 4px rgba(250,204,21,.12)' });

    const frame = document.createElement('div');
    frame.id = frameId;
    frame.setAttribute('aria-hidden', 'true');
    Object.assign(frame.style, {
      position: 'fixed', inset: '0', zIndex: '2147483645', pointerEvents: 'none', boxSizing: 'border-box',
      border: '2px solid rgba(168,85,247,.95)', borderRadius: '2px',
      background: 'linear-gradient(to bottom,rgba(168,85,247,.17),transparent 13%),linear-gradient(to top,rgba(59,130,246,.15),transparent 13%),linear-gradient(to right,rgba(168,85,247,.14),transparent 11%),linear-gradient(to left,rgba(250,204,21,.12),transparent 11%)',
      boxShadow: '0 0 0 1px rgba(255,255,255,.12) inset,0 0 42px 12px rgba(168,85,247,.26) inset,0 0 90px 26px rgba(59,130,246,.12) inset'
    });

    banner.animate([
      { opacity: 0, transform: 'translateY(-18px) scale(.96)' },
      { opacity: 1, transform: 'translateY(2px) scale(1.012)', offset: .72 },
      { opacity: 1, transform: 'translateY(0) scale(1)' }
    ], { duration: 620, easing: 'cubic-bezier(.16,1,.3,1)' });
    banner.animate([
      { boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 22px rgba(250,204,21,.14)' },
      { boxShadow: '0 28px 92px rgba(15,23,42,.56),0 0 0 1px rgba(255,255,255,.12) inset,0 0 52px rgba(250,204,21,.34)' },
      { boxShadow: '0 24px 80px rgba(15,23,42,.5),0 0 0 1px rgba(255,255,255,.08) inset,0 0 22px rgba(250,204,21,.14)' }
    ], { duration: 2100, iterations: Infinity, easing: 'ease-in-out' });
    icon.animate([{ transform: 'scale(1)' }, { transform: 'scale(1.1)' }, { transform: 'scale(1)' }], { duration: 1250, iterations: Infinity, easing: 'ease-in-out' });
    dot.animate([{ opacity: .35, transform: 'scale(.8)' }, { opacity: 1, transform: 'scale(1.25)' }, { opacity: .35, transform: 'scale(.8)' }], { duration: 820, iterations: Infinity, easing: 'ease-in-out' });
    frame.animate([
      {
        borderColor: 'rgba(168,85,247,.6)',
        background: 'linear-gradient(to bottom,rgba(168,85,247,.1),transparent 11%),linear-gradient(to top,rgba(59,130,246,.09),transparent 11%),linear-gradient(to right,rgba(168,85,247,.08),transparent 9%),linear-gradient(to left,rgba(250,204,21,.07),transparent 9%)',
        boxShadow: '0 0 0 1px rgba(255,255,255,.08) inset,0 0 28px 7px rgba(168,85,247,.18) inset,0 0 60px 18px rgba(59,130,246,.08) inset'
      },
      {
        borderColor: 'rgba(250,204,21,.98)',
        background: 'linear-gradient(to bottom,rgba(250,204,21,.22),transparent 18%),linear-gradient(to top,rgba(168,85,247,.22),transparent 18%),linear-gradient(to right,rgba(59,130,246,.18),transparent 15%),linear-gradient(to left,rgba(250,204,21,.17),transparent 15%)',
        boxShadow: '0 0 0 1px rgba(255,255,255,.16) inset,0 0 58px 18px rgba(168,85,247,.4) inset,0 0 120px 38px rgba(59,130,246,.18) inset'
      },
      {
        borderColor: 'rgba(59,130,246,.9)',
        background: 'linear-gradient(to bottom,rgba(59,130,246,.18),transparent 15%),linear-gradient(to top,rgba(168,85,247,.2),transparent 16%),linear-gradient(to right,rgba(250,204,21,.13),transparent 12%),linear-gradient(to left,rgba(59,130,246,.16),transparent 13%)',
        boxShadow: '0 0 0 1px rgba(255,255,255,.12) inset,0 0 48px 14px rgba(59,130,246,.34) inset,0 0 105px 31px rgba(168,85,247,.15) inset'
      }
    ], { duration: 1650, iterations: Infinity, direction: 'alternate', easing: 'ease-in-out' });

    document.documentElement.append(frame, banner);
  }

  globalThis.ChatCmdConversationUi = Object.freeze({ renderReturnToChatCmd });
})();
