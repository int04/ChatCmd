import { Ban, Check, CheckCheck, CircleAlert, LoaderCircle, X } from 'lucide-react';
import { FormEvent, useEffect, useId, useRef, useState } from 'react';
import { api } from '../api';
import { tr } from '../i18n';

export interface ApprovalDecisionTarget { taskId: string; activityId: string; turnId?: string; }

export function ApprovalDecisionActions({ target, onResolved, reusable = true }: { target: ApprovalDecisionTarget; onResolved?: () => void; reusable?: boolean }) {
  const [busy, setBusy] = useState<'allow' | 'allowSimilar' | 'reject' | null>(null); const [error, setError] = useState(''); const [rejecting, setRejecting] = useState(false);
  const resolve = async (decision: 'allow' | 'allowSimilar' | 'reject', reason?: string) => {
    if (busy) return; setBusy(decision); setError('');
    try { await api.resolveTaskApproval(target.taskId, target.activityId, { turnId: target.turnId, decision, reason }); setRejecting(false); onResolved?.(); }
    catch (failure) { setError(failure instanceof Error ? failure.message : tr('Could not send approval decision.')); setBusy(null); }
  };
  return <><div className="approval-actions" role="group" aria-label={tr('Approval decision')}>
    <button type="button" className="approval-allow" disabled={Boolean(busy)} onClick={() => void resolve('allow')}>{busy === 'allow' ? <LoaderCircle className="spin" /> : <Check />}{tr('Allow')}</button>
    {reusable && <button type="button" className="approval-similar" disabled={Boolean(busy)} onClick={() => void resolve('allowSimilar')}>{busy === 'allowSimilar' ? <LoaderCircle className="spin" /> : <CheckCheck />}{tr('Allow similar')}</button>}
    <button type="button" className="approval-reject" disabled={Boolean(busy)} onClick={() => setRejecting(true)}><Ban />{tr('Reject')}</button>{error && <p role="alert"><CircleAlert />{error}</p>}
  </div>{rejecting && <RejectApprovalDialog busy={busy === 'reject'} error={error} close={() => !busy && setRejecting(false)} reject={(reason) => resolve('reject', reason)} />}</>;
}

function RejectApprovalDialog({ busy, error, close, reject }: { busy: boolean; error: string; close: () => void; reject: (reason?: string) => Promise<void> }) {
  const [reason, setReason] = useState(''); const titleId = useId(); const descriptionId = useId(); const dialogRef = useRef<HTMLDivElement>(null); const reasonRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null; reasonRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape' && !busy) { event.preventDefault(); close(); return; } if (event.key !== 'Tab') return; const focusable = [...dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), textarea:not(:disabled)') ?? []]; if (!focusable.length) return; const first = focusable[0]; const last = focusable.at(-1)!; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } };
    document.addEventListener('keydown', onKeyDown); return () => { document.removeEventListener('keydown', onKeyDown); previous?.focus(); };
  }, [busy, close]);
  const submit = (event: FormEvent) => { event.preventDefault(); if (!busy) void reject(reason.trim() || undefined); };
  return <div className="modal-backdrop approval-reject-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) close(); }}><form onSubmit={submit}><div ref={dialogRef} className="modal compact-modal approval-reject-modal" role="alertdialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={descriptionId} aria-busy={busy}>
    <header><div><h2 id={titleId}>{tr('Reject permission request?')}</h2><p id={descriptionId}>{tr('The reason will be sent back to the Agent so it knows why the command was not allowed.')}</p></div><button type="button" className="icon-button" aria-label={tr('Close')} disabled={busy} onClick={close}><X /></button></header>
    <label htmlFor={`${titleId}-reason`}>{tr('Rejection reason (optional)')}</label><textarea ref={reasonRef} id={`${titleId}-reason`} value={reason} maxLength={2000} rows={4} disabled={busy} placeholder={tr('Example: Do not run this command; use a read-only approach instead.')} onChange={(event) => setReason(event.target.value)} />
    <div className="stop-reason-meta"><span>{tr('This reason will be sent back to the AI.')}</span><span>{reason.length}/2000</span></div>{error && <p className="stop-activity-error" role="alert"><CircleAlert />{error}</p>}
    <div className="modal-actions"><button type="button" className="button secondary" disabled={busy} onClick={close}>{tr('Cancel')}</button><button type="submit" className="button danger" disabled={busy}>{busy ? <LoaderCircle className="spin" /> : <Ban />}{busy ? tr('Rejecting…') : tr('Reject')}</button></div>
  </div></form></div>;
}
