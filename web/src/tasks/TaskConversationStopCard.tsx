import { CircleAlert, CircleStop, LoaderCircle, OctagonX } from 'lucide-react';
import { type FormEvent, useEffect, useRef, useState } from 'react';

import { api } from '../api';
import type { TaskDetail } from '../types';

const labels = {
  button: 'Ngừng trò chuyện này',
  title: 'Bạn có muốn ngừng cuộc trò chuyện này không?',
  body: 'Phiên làm việc này sẽ kết thúc.',
  cancel: 'Hủy',
  confirm: 'Ngừng trò chuyện',
  stopping: 'Đang ngừng…',
  stopped: 'Cuộc trò chuyện đã ngừng',
  error: 'Không thể ngừng cuộc trò chuyện. Hãy thử lại.',
} as const;

export function TaskConversationStopCard({ taskId, taskStatus, onStopped }: { taskId: string; taskStatus: string; onStopped: (detail: TaskDetail) => void }) {
  const stoppedByServer = isStoppedStatus(taskStatus);
  const [confirming, setConfirming] = useState(false);
  const [stopped, setStopped] = useState(stoppedByServer);

  useEffect(() => {
    setStopped(stoppedByServer);
    setConfirming(false);
  }, [stoppedByServer, taskId]);

  return <section className="task-stop-card">
    <button type="button" className="task-stop-button" disabled={stopped} onClick={() => setConfirming(true)}>
      {stopped ? <OctagonX aria-hidden="true" /> : <CircleStop aria-hidden="true" />}
      {stopped ? labels.stopped : labels.button}
    </button>
    {confirming && <StopConversationDialog taskId={taskId} onClose={() => setConfirming(false)} onStopped={(detail) => { setStopped(true); setConfirming(false); onStopped(detail); }} />}
  </section>;
}

function StopConversationDialog({ taskId, onClose, onStopped }: { taskId: string; onClose: () => void; onStopped: (detail: TaskDetail) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = `stop-conversation-title-${taskId}`;
  const descriptionId = `stop-conversation-description-${taskId}`;

  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const frame = window.requestAnimationFrame(() => cancelRef.current?.focus());
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) { event.preventDefault(); onClose(); return; }
      if (event.key !== 'Tab') return;
      const focusable = [...(dialogRef.current?.querySelectorAll<HTMLButtonElement>('button:not([disabled])') ?? [])];
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable.at(-1)!;
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener('keydown', onKeyDown);
      previousFocus?.focus();
    };
  }, [busy, onClose]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      onStopped(await api.stopTask(taskId));
    } catch (reason) {
      setError(reason instanceof Error && reason.message ? reason.message : labels.error);
      setBusy(false);
    }
  }

  return <div className="modal-backdrop stop-conversation-backdrop" onClick={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}>
    <form onSubmit={submit}>
      <div ref={dialogRef} className="modal compact-modal stop-conversation-modal" role="dialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={descriptionId} aria-busy={busy}>
        <div className="stop-conversation-header"><span className="stop-conversation-icon"><CircleStop aria-hidden="true" /></span><div><h2 id={titleId}>{labels.title}</h2><p id={descriptionId}>{labels.body}</p></div></div>
        {error && <p className="stop-conversation-error" role="alert"><CircleAlert aria-hidden="true" />{error}</p>}
        <div className="modal-actions">
          <button ref={cancelRef} type="button" className="button secondary" disabled={busy} onClick={onClose}>{labels.cancel}</button>
          <button type="submit" className="button danger" disabled={busy}>{busy ? <LoaderCircle className="spin" aria-hidden="true" /> : <CircleStop aria-hidden="true" />}{busy ? labels.stopping : labels.confirm}</button>
        </div>
      </div>
    </form>
  </div>;
}

function isStoppedStatus(status: string) {
  const normalized = status.toLowerCase();
  return normalized === 'stopped' || normalized === 'cancelled' || normalized === 'canceled';
}
