import { BrainCircuit, Cpu, LoaderCircle } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
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
      {browser.messages.length ? browser.messages.map((message) => <div className={`turn-browser-message ${message.kind}`} key={message.id}>
        <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]} components={{ a: ({ children: text, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener">{text}</a> }}>{message.content}</ReactMarkdown>
      </div>) : <div className="turn-source-empty" role="status">
        {running && <LoaderCircle className="spin" aria-hidden="true" />}
        <span>{vi ? (running ? 'Đang chờ nội dung hiển thị từ ChatGPT…' : 'Lượt này chưa có bản ghi từ trình duyệt.')
          : (running ? 'Waiting for visible ChatGPT content…' : 'No browser transcript was recorded for this turn.')}</span>
      </div>}
    </section> : <section aria-label="ChatCMD Think">{children}</section>}
  </div>;
}
