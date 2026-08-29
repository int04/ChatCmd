import { Clock3, ListChecks } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../api';
import { Modal } from '../components';
import { tr } from '../i18n';
import { useRealtime } from '../realtime';
import { soundNotifications } from '../soundNotifications';
import type { Task, TimelineEvent } from '../types';

function sortQueue(items: Task[]) {
  return [...items].sort((left, right) => {
    const leftTime = Date.parse(left.createdAtUtc ?? left.updatedAtUtc);
    const rightTime = Date.parse(right.createdAtUtc ?? right.updatedAtUtc);
    return leftTime - rightTime || left.id.localeCompare(right.id);
  });
}

export function GlobalConversationApprovalQueue() {
  const [queue, setQueue] = useState<Task[]>([]);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');
  const [remaining, setRemaining] = useState(0);
  const current = queue[0];
  const activeSoundTask = useRef<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const items = await api.pendingConversationApprovals();
      setQueue(sortQueue(items.filter((item) => item.allowExecute === null)));
    } catch { /* websocket/polling will retry */ }
  }, []);

  useEffect(() => { void reload(); }, [reload]);
  useEffect(() => {
    const timer = window.setInterval(() => void reload(), 1500);
    return () => window.clearInterval(timer);
  }, [reload]);

  const onRealtime = useCallback((event: TimelineEvent) => {
    if (event.type === 'conversation.approval_pending' && event.taskId) {
      void api.task(event.taskId).then((detail) => {
        setQueue((items) => sortQueue(items.some((item) => item.id === detail.task.id) ? items : [...items, detail.task]));
      }).catch(() => void reload());
      return;
    }
    if (event.type === 'conversation.approval_resolved' && event.taskId) {
      setQueue((items) => items.filter((item) => item.id !== event.taskId));
    }
  }, [reload]);
  useRealtime(onRealtime);

  const deadline = useMemo(() => {
    if (!current) return 0;
    const explicit = current.approvalDeadlineUtc ? Date.parse(current.approvalDeadlineUtc) : NaN;
    if (Number.isFinite(explicit)) return explicit;
    return Date.parse(current.createdAtUtc ?? current.updatedAtUtc) + 60_000;
  }, [current]);

  useEffect(() => {
    if (!current) { setRemaining(0); return; }
    const tick = () => setRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    tick();
    const timer = window.setInterval(tick, 250);
    return () => window.clearInterval(timer);
  }, [current, deadline]);

  useEffect(() => {
    if (!current) {
      activeSoundTask.current = null;
      return;
    }
    if (activeSoundTask.current === current.id) return;
    activeSoundTask.current = current.id;
    soundNotifications.playApproval();
  }, [current]);

  useEffect(() => {
    if (!current) return;
    const previous = document.title;
    document.documentElement.dataset.approvalRequired = 'true';
    document.title = 'Xin phê duyệt';
    return () => {
      delete document.documentElement.dataset.approvalRequired;
      document.title = previous;
    };
  }, [current]);

  const decide = useCallback(async (approved: boolean) => {
    if (!current || busy) return;
    setBusy(true);
    setProblem('');
    try {
      await api.taskAction(current.id, approved ? 'approve-execution' : 'reject-execution');
      setQueue((items) => items.filter((item) => item.id !== current.id));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : tr('Could not send conversation approval decision.'));
      await reload();
    } finally {
      setBusy(false);
    }
  }, [busy, current, reload]);

  if (!current) return null;

  return <Modal title="Xin phê duyệt" description={tr('A new ChatGPT Website conversation is waiting for approval before the Agent can execute anything.')} close={() => void decide(false)} dangerous>
    <div className="warning-block"><ListChecks /><p>{tr('{count} conversation(s) waiting. Requests are shown one at a time so none are missed.', { count: queue.length })}</p></div>
    <div className="warning-block"><Clock3 /><p>{tr('Approval expires in {seconds} seconds.', { seconds: remaining })}</p></div>
    <p><strong>{current.title?.trim() || current.id}</strong></p>
    {problem && <p role="alert">{problem}</p>}
    <div className="modal-actions"><button className="button danger" type="button" disabled={busy} onClick={() => void decide(false)}>{tr('Reject')}</button><button className="button primary" type="button" disabled={busy || remaining === 0} onClick={() => void decide(true)}>{tr('Approve')}</button></div>
  </Modal>;
}
