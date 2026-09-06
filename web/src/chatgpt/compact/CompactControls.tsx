import { Check, CircleAlert, Layers3, LoaderCircle, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import { createPortal } from 'react-dom';
import { Modal } from '../../components';
import { tr } from '../../i18n';
import { useCompact } from './CompactProvider';
import { compactText } from './copy';
import { compactConfirmation, compactReferenceUrl, compactSteps, type CompactPhase } from './types';

export function CompactAction({ disabled = false }: { disabled?: boolean }) {
  const compact = useCompact();
  const [confirming, setConfirming] = useState(false);
  const [continueAfterCompact, setContinueAfterCompact] = useState(false);
  if (!compact) return null;
  const unavailable = disabled || compact.blocked;
  return <>
    <button type="button" className="compact-action" disabled={unavailable} onClick={() => { setContinueAfterCompact(false); setConfirming(true); }}>
      <Layers3 aria-hidden="true" />Compact &amp; resume now
    </button>
    {/* Keep the confirmation outside the footer's clipping/stacking context and composer form. */}
    {confirming && createPortal(<Modal title="Compact & resume now" description={compactConfirmation} className="compact-confirmation" close={() => setConfirming(false)}>
      <label className="compact-continue-option">
        <input type="checkbox" checked={continueAfterCompact} disabled={unavailable} aria-describedby="compact-continue-hint" onChange={(event) => setContinueAfterCompact(event.target.checked)} />
        <span>{compactText('continueAfterCompact')}</span>
      </label>
      <p id="compact-continue-hint" className="compact-continue-hint">{compactText('continueHint')}</p>
      <div className="modal-actions">
        <button type="button" className="button secondary" onClick={() => setConfirming(false)}>{tr('Cancel')}</button>
        <button type="button" className="button primary" disabled={unavailable} onClick={() => {
          if (unavailable) return;
          setConfirming(false);
          void compact.create(continueAfterCompact);
        }}>{compactText('confirm')}</button>
      </div>
    </Modal>, document.body)}
  </>;
}

export function CompactStatusCard() {
  const compact = useCompact();
  if (!compact) return null;
  const job = compact.active;
  // Completed/cancelled jobs belong in history, not in an always-visible progress panel.
  if (!job) return compact.error ? <section aria-label="Compact & resume error">
    <p className="compact-error" role="alert"><CircleAlert aria-hidden="true" />{compact.error}</p>
    <button type="button" className="button secondary" onClick={() => void compact.refresh(true)}>{tr('Retry')}</button>
  </section> : null;
  const phase = job?.phase ?? 'preparing';
  const activeIndex = compactSteps.findIndex((step) => step.phase === phase);
  const terminal = phase === 'completed' || phase === 'cancelled';
  const currentLabel = phaseLabel(phase);
  const detail = !job ? compactText('checking') : job.detail || compactText(phase === 'completed' ? 'completedDetail' : phase === 'cancelled' ? 'cancelledDetail' : phase);
  return <section className={`compact-status-card ${phase}`} aria-label="Compact & resume progress">
    <header className="compact-status-heading">
      <span className="compact-status-icon" aria-hidden="true">{phase === 'completed' ? <Check /> : terminal ? <Layers3 /> : <LoaderCircle className="compact-spinner" />}</span>
      <div><h3>ChatGPT is writing the handoff</h3><span>{terminal ? currentLabel : compactText('preserved')}</span></div>
    </header>
    <ol className="compact-steps" aria-label="Compact & resume steps">
      {compactSteps.map((step, index) => {
        const done = phase === 'completed' || (!terminal && index < activeIndex);
        const current = !terminal && index === activeIndex;
        return <li key={step.phase} data-state={done ? 'done' : current ? 'current' : 'pending'} aria-current={current ? 'step' : undefined}>
          <span className="compact-step-marker" aria-hidden="true">{done ? <Check /> : index + 1}</span>
          <span>{step.label}{done && <span className="sr-only"> — {compactText('completed')}</span>}</span>
        </li>;
      })}
    </ol>
    <div className="compact-status-detail" role="status" aria-live="polite" aria-atomic="true">
      <strong>{currentLabel}</strong><p>{detail}</p>
      {!terminal && <p>{compact.extensionMessage || compactText('waiting')}</p>}
    </div>
    {compact.error && <p className="compact-error" role="alert"><CircleAlert aria-hidden="true" />{compact.error}</p>}
    <div className="compact-status-actions">
      {compact.active && <>
        {compactReferenceUrl(compact.active.oldConversationUrl) && <a className="button secondary" href={compactReferenceUrl(compact.active.oldConversationUrl)!} target="_blank" rel="noopener noreferrer">Mở lại chat ChatGPT cũ</a>}
        {compactReferenceUrl(compact.active.newConversationUrl ?? '') && <a className="button secondary" href={compactReferenceUrl(compact.active.newConversationUrl ?? '')!} target="_blank" rel="noopener noreferrer">Mở lại chat ChatGPT mới</a>}
        <button type="button" className="button secondary" disabled={compact.busy || compact.waking} onClick={() => void compact.resume()}>
          <RefreshCw aria-hidden="true" />{compactText('resume')}
        </button>
        <button type="button" className="button secondary" disabled={compact.busy} onClick={() => void compact.cancel()}>{compactText('cancelJob')}</button>
      </>}
      {!compact.active && compact.error && <button type="button" className="button secondary" onClick={() => void compact.refresh(true)}>{tr('Retry')}</button>}
    </div>
  </section>;
}

export function phaseLabel(phase: CompactPhase): string {
  if (phase === 'completed' || phase === 'cancelled') return compactText(phase);
  return compactSteps.find((step) => step.phase === phase)?.label ?? phase;
}
