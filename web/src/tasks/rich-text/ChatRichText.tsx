import { FileText, Mail } from 'lucide-react';
import { lazy, Suspense, useId, useMemo } from 'react';
import { appLocale } from '../../i18n';
import { CopyTextButton } from './CopyTextButton';
import { MarkdownContent } from './MarkdownContent';
import { richTextLabels } from './labels';
import { normalizeChatMarkup } from './normalizeMarkup';
import { splitWritingBlocks, type WritingPart } from './writingBlocks';
import './chatRichText.css';

const MathMarkdown = lazy(() => import('./MathMarkdown'));

function Markdown({ content, vi }: { content: string; vi: boolean }) {
  const plain = <MarkdownContent content={content} vi={vi} />;
  return /(^|[^\\])\$|language-math/m.test(content)
    ? <Suspense fallback={plain}><MathMarkdown content={content} vi={vi} /></Suspense>
    : plain;
}

function WritingCard({ part, markdown, vi }: { part: WritingPart; markdown: string; vi: boolean }) {
  const titleId = useId();
  const labels = richTextLabels(vi);
  const email = part.attributes.variant?.toLowerCase() === 'email';
  const title = part.attributes.title || part.attributes.subject || (email ? labels.email : labels.document);
  const recipient = part.attributes.recipient || part.attributes.to;
  const Icon = email ? Mail : FileText;
  return <section className="chat-writing-card" aria-labelledby={titleId} data-writing-id={part.attributes.id}>
    <header className="chat-writing-header">
      <span className="chat-writing-icon"><Icon aria-hidden="true" /></span>
      <div className="chat-writing-heading"><small>{email ? labels.email : labels.document}</small><h4 id={titleId}>{title}</h4></div>
      <CopyTextButton text={part.content} labels={labels} />
    </header>
    {email && recipient && <p className="chat-writing-recipient"><strong>{labels.recipient}:</strong> {recipient}</p>}
    <div className="chat-writing-body"><Markdown content={markdown} vi={vi} /></div>
  </section>;
}

/** One sanitized renderer for user messages, progress, browser snapshots and final responses. */
export function ChatRichText({ content }: { content: string }) {
  const vi = appLocale().startsWith('vi');
  const parts = useMemo(() => {
    const references = new Map<string, number>();
    return splitWritingBlocks(content).map((part) => ({ part, markdown: normalizeChatMarkup(part.content, vi, references) }));
  }, [content, vi]);
  return <div className="chat-rich-text">{parts.map(({ part, markdown }) => part.kind === 'writing'
    ? <WritingCard part={part} markdown={markdown} vi={vi} key={part.start} />
    : <Markdown content={markdown} vi={vi} key={part.start} />)}</div>;
}
