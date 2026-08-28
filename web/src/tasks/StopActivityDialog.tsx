import { CircleAlert, CircleStop, LoaderCircle } from 'lucide-react';
import { type FormEvent, useEffect, useRef, useState } from 'react';
import { api } from '../api';
import { tr } from '../i18n';
import type { ToolActivity } from './taskTimeline';

export function StopActivityDialog({ taskId, activity, onClose }: { taskId: string; activity: ToolActivity; onClose: () => void }) {
  const [reason, setReason] = useState(''); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  const dialogRef = useRef<HTMLDivElement>(null); const reasonRef = useRef<HTMLTextAreaElement>(null);
  const titleId = `stop-activity-title-${activity.id}`; const descriptionId = `stop-activity-description-${activity.id}`; const reasonId = `stop-activity-reason-${activity.id}`;
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null; const frame = window.requestAnimationFrame(() => reasonRef.current?.focus());
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape' && !busy) { event.preventDefault(); onClose(); return; } if (event.key !== 'Tab') return; const focusable = [...(dialogRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), textarea:not([disabled])') ?? [])]; if (!focusable.length) return; const first = focusable[0]; const last = focusable.at(-1)!; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } };
    document.addEventListener('keydown', onKeyDown); return () => { window.cancelAnimationFrame(frame); document.removeEventListener('keydown', onKeyDown); previousFocus?.focus(); };
  }, [busy, onClose]);
  async function submit(event: FormEvent) {
    event.preventDefault(); if (busy) return; setBusy(true); setError('');
    try { await api.stopTaskActivity(taskId, activity.id, { turnId: activity.turnId, reason: reason.trim() || undefined }); onClose(); }
    catch (requestError) { setError(requestError instanceof Error && requestError.message ? requestError.message : tr('Could not stop this activity. Please try again.')); setBusy(false); }
  }
  return <div className="modal-backdrop stop-activity-backdrop" onClick={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><form onSubmit={submit}><div ref={dialogRef} className="modal compact-modal stop-activity-modal" role="dialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={descriptionId} aria-busy={busy}>
    <div className="stop-activity-header"><span className="stop-activity-icon"><CircleStop aria-hidden="true" /></span><div><h2 id={titleId}>{tr('Stop this activity?')}</h2><p id={descriptionId}>{tr('The running tool will be stopped. If provided, the reason will be sent back to the Agent.')}</p></div></div>
    <label className="stop-reason-label" htmlFor={reasonId}>{tr('Stop reason (optional)')}</label><textarea ref={reasonRef} id={reasonId} value={reason} maxLength={2000} rows={4} disabled={busy} placeholder={tr('Example: Stop this command and use another approach…')} aria-describedby={`${reasonId}-hint`} onChange={(event) => setReason(event.target.value)} />
    <div className="stop-reason-meta" id={`${reasonId}-hint`}><span>{tr('Leave blank if you only want the Agent to stop the current activity.')}</span><span aria-label={`${reason.length}/2000`}>{reason.length}/2000</span></div>
    {error && <p className="stop-activity-error" role="alert"><CircleAlert aria-hidden="true" />{error}</p>}
    <div className="modal-actions"><button type="button" className="button secondary" disabled={busy} onClick={onClose}>{tr('Cancel')}</button><button type="submit" className="button danger" disabled={busy}>{busy ? <LoaderCircle className="spin" aria-hidden="true" /> : <CircleStop aria-hidden="true" />}{busy ? tr('Stopping…') : tr('Stop activity')}</button></div>
  </div></form></div>;
}
