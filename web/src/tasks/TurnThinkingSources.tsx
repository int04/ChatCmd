import { BrainCircuit, Cpu, LoaderCircle } from 'lucide-react';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { ChatRichText } from './rich-text/ChatRichText';
import { appLocale } from '../i18n';
import type { BrowserThinking } from './chatGptThinking';
import './turnThinkingSources.css';

type Source = 'chatgpt' | 'chatcmd';
export function TurnThinkingSources({ browser, hasMcp, running, children, enabled = true }: {
  browser: BrowserThinking; hasMcp: boolean; running: boolean; children: ReactNode; enabled?: boolean;
}) {
  const [choice, setChoice] = useState<{ source: Source; hadMcp: boolean } | null>(null);
  // The first meaningful MCP event switches the provisional view once. Subsequent user choices stick.
  const source = hasMcp ? (choice?.hadMcp ? choice.source : 'chatcmd') : 'chatgpt';
  const vi = appLocale().startsWith('vi');
  if (!enabled) return <>{children}</>;
  return <div className="turn-thinking-sources">
    <div className="turn-thinking-source-switch" role="group" aria-label={vi ? 'Nguồn nội dung' : 'Thinking source'}>
      <button type="button" aria-pressed={source === 'chatgpt'} onClick={() => setChoice({ source: 'chatgpt', hadMcp: hasMcp })}>
        <BrainCircuit aria-hidden="true" /><span>ChatGPT Think</span>
        {running && !browser.completed && <span className="turn-source-live" aria-label={vi ? 'Đang nhận' : 'Receiving'} />}
      </button>
      <button type="button" aria-pressed={source === 'chatcmd'} disabled={!hasMcp} onClick={() => setChoice({ source: 'chatcmd', hadMcp: hasMcp })}>
        <Cpu aria-hidden="true" /><span>ChatCMD Think</span>
      </button>
    </div>
    {source === 'chatgpt' ? <section className="turn-browser-thinking" aria-label="ChatGPT Think">
      <p className="turn-source-caption">{vi
        ? (hasMcp ? 'Nội dung ChatGPT đã hiển thị trên trang, được lưu riêng với MCP.' : 'Hiển thị từ ChatGPT trong khi chưa có nội dung MCP. Bản ghi này vẫn được giữ lại.')
        : (hasMcp ? 'Public ChatGPT page content, saved separately from MCP.' : 'Showing ChatGPT while no MCP content is available. This transcript is retained.')}</p>
      {browser.messages.length ? browser.messages.map((message, index) => {
        const live = running && !browser.completed && index === browser.messages.length - 1;
        return <SmoothBrowserMessage key={message.id} kind={message.kind} content={message.content} live={live} revision={browser.revision} />;
      }) : <div className="turn-source-empty" role="status">
        {running && <LoaderCircle className="spin" aria-hidden="true" />}
        <span>{vi ? (running ? 'Đang chờ nội dung hiển thị từ ChatGPT…' : 'Lượt này chưa có bản ghi từ trình duyệt.')
          : (running ? 'Waiting for visible ChatGPT content…' : 'No browser transcript was recorded for this turn.')}</span>
      </div>}
    </section> : <section aria-label="ChatCMD Think">{children}</section>}
  </div>;
}

function SmoothBrowserMessage({ kind, content, live, revision }: { kind: 'commentary' | 'answer'; content: string; live: boolean; revision: number }) {
  const innerRef = useRef<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    const node = innerRef.current;
    if (!node || !live || typeof node.animate !== 'function' || window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return;
    node.animate(
      [{ opacity: 0.9, transform: 'translateY(1px)' }, { opacity: 1, transform: 'translateY(0)' }],
      { duration: 170, easing: 'cubic-bezier(.16,1,.3,1)' },
    );
  }, [content, live, revision]);

  return <div className="turn-browser-message-shell">
    <div ref={innerRef} className={`turn-browser-message ${kind}${live ? ' live' : ''}`}>
      <ChatRichText content={content} />
    </div>
  </div>;
}
