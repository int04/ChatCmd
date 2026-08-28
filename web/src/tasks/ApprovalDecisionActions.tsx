import { Ban, Check, CheckCheck, CircleAlert, LoaderCircle, X } from 'lucide-react';
import { FormEvent, useEffect, useId, useRef, useState } from 'react';

import { api } from '../api';

export interface ApprovalDecisionTarget {
  taskId: string;
  activityId: string;
  turnId?: string;
}

export function ApprovalDecisionActions({ target, onResolved }: { target: ApprovalDecisionTarget; onResolved?: () => void }) {
  const [busy, setBusy] = useState<'allow' | 'allowSimilar' | 'reject' | null>(null);
  const [error, setError] = useState('');
  const [rejecting, setRejecting] = useState(false);

  const resolve = async (decision: 'allow' | 'allowSimilar' | 'reject', reason?: string) => {
    if (busy) return;
    setBusy(decision);
    setError('');
    try {
      await api.resolveTaskApproval(target.taskId, target.activityId, { turnId: target.turnId, decision, reason });
      setRejecting(false);
      onResolved?.();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'Không thể gửi quyết định phê duyệt.');
      setBusy(null);
    }
  };

  return <>
    <div className="approval-actions" role="group" aria-label="Quyết định quyền chạy lệnh">
      <button type="button" className="approval-allow" disabled={Boolean(busy)} onClick={() => void resolve('allow')}>{busy === 'allow' ? <LoaderCircle className="spin" /> : <Check />}Chấp nhận</button>
      <button type="button" className="approval-similar" disabled={Boolean(busy)} onClick={() => void resolve('allowSimilar')}>{busy === 'allowSimilar' ? <LoaderCircle className="spin" /> : <CheckCheck />}Chấp nhận tương tự</button>
      <button type="button" className="approval-reject" disabled={Boolean(busy)} onClick={() => setRejecting(true)}><Ban />Từ chối</button>
      {error && <p role="alert"><CircleAlert />{error}</p>}
    </div>
    {rejecting && <RejectApprovalDialog busy={busy === 'reject'} error={error} close={() => !busy && setRejecting(false)} reject={(reason) => resolve('reject', reason)} />}
  </>;
}

function RejectApprovalDialog({ busy, error, close, reject }: { busy: boolean; error: string; close: () => void; reject: (reason?: string) => Promise<void> }) {
  const [reason, setReason] = useState('');
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const reasonRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    reasonRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) { event.preventDefault(); close(); return; }
      if (event.key !== 'Tab') return;
      const focusable = [...dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), textarea:not(:disabled)') ?? []];
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable.at(-1)!;
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => { document.removeEventListener('keydown', onKeyDown); previous?.focus(); };
  }, [busy, close]);

  const submit = (event: FormEvent) => { event.preventDefault(); if (!busy) void reject(reason.trim() || undefined); };
  return <div className="modal-backdrop approval-reject-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) close(); }}>
    <form onSubmit={submit}>
      <div ref={dialogRef} className="modal compact-modal approval-reject-modal" role="alertdialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={descriptionId} aria-busy={busy}>
        <header><div><h2 id={titleId}>Từ chối yêu cầu quyền?</h2><p id={descriptionId}>Lý do sẽ được gửi lại cho Agent để Agent biết vì sao lệnh không được phép chạy.</p></div><button type="button" className="icon-button" aria-label="Đóng" disabled={busy} onClick={close}><X /></button></header>
        <label htmlFor={`${titleId}-reason`}>Lý do từ chối (không bắt buộc)</label>
        <textarea ref={reasonRef} id={`${titleId}-reason`} value={reason} maxLength={2000} rows={4} disabled={busy} placeholder="Ví dụ: Không chạy lệnh này; hãy dùng cách chỉ đọc dữ liệu." onChange={(event) => setReason(event.target.value)} />
        <div className="stop-reason-meta"><span>Lý do này sẽ được callback về AI.</span><span>{reason.length}/2000</span></div>
        {error && <p className="stop-activity-error" role="alert"><CircleAlert />{error}</p>}
        <div className="modal-actions"><button type="button" className="button secondary" disabled={busy} onClick={close}>Hủy</button><button type="submit" className="button danger" disabled={busy}>{busy ? <LoaderCircle className="spin" /> : <Ban />}{busy ? 'Đang từ chối…' : 'Từ chối'}</button></div>
      </div>
    </form>
  </div>;
}
