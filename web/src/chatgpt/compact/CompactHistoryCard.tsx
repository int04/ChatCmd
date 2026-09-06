import { ExternalLink, History, Layers3 } from 'lucide-react';
import { formatTime } from '../../components';
import { tr } from '../../i18n';
import { useCompact } from './CompactProvider';
import { phaseLabel } from './CompactControls';
import { compactText } from './copy';
import { compactReferenceUrl } from './types';

export function CompactHistoryCard() {
  const compact = useCompact();
  if (!compact) return null;
  return <section className="compact-history-card" aria-label="Lịch sử thu gọn ngữ cảnh">
    <header><History aria-hidden="true" /><h2>Lịch sử thu gọn ngữ cảnh</h2><span className="compact-history-count">{compact.history.length}</span></header>
    {!compact.ready && !compact.error && <p className="compact-history-empty">{compactText('checking')}</p>}
    {compact.ready && compact.history.length === 0 && <div className="compact-history-empty"><Layers3 aria-hidden="true" /><p>{compactText('empty')}</p></div>}
    {compact.error && <div className="compact-history-error"><p>{compact.error}</p><button type="button" onClick={() => void compact.refresh()}>{tr('Retry')}</button></div>}
    <ol className="compact-history-list">
      {compact.history.map((job) => {
        const oldUrl = compactReferenceUrl(job.oldConversationUrl);
        const at = Number.isFinite(job.createdAtMs) ? new Date(job.createdAtMs).toISOString() : undefined;
        return <li key={job.id} className="compact-history-entry">
          <div className="compact-history-meta"><span className={`compact-history-state ${job.phase}`}>{phaseLabel(job.phase)}</span><time dateTime={at}>{formatTime(at)}</time></div>
          {oldUrl ? <a href={oldUrl} target="_blank" rel="noopener noreferrer">
            <ExternalLink aria-hidden="true" /><span>{compactText('reference')}<span className="sr-only"> {compactText('newTab')}</span></span>
          </a> : <p>{compactText('noUrl')}</p>}
          <details><summary>{compactText('details')}</summary><dl>
            <div><dt>{compactText('oldId')}</dt><dd>{job.oldConversationId || '—'}</dd></div>
            <div><dt>{compactText('newId')}</dt><dd>{job.newConversationId || '—'}</dd></div>
            <div><dt>{tr('Model')}</dt><dd>{job.oldModel || '—'}</dd></div>
          </dl>{job.detail && <p>{job.detail}</p>}</details>
        </li>;
      })}
    </ol>
  </section>;
}
