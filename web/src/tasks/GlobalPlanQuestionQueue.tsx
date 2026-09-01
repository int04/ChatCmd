import { Clock3, ListChecks, MessageSquareMore, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ApiError, api } from '../api';
import { Modal } from '../components';
import { tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { PlanQuestion, TimelineEvent } from '../types';

function sortQueue(items: PlanQuestion[]) {
  return [...items].sort((left, right) => left.createdAtMs - right.createdAtMs || left.id.localeCompare(right.id));
}

function formatRemaining(seconds: number) {
  const minutes = Math.floor(seconds / 60).toString().padStart(2, '0');
  const remainder = (seconds % 60).toString().padStart(2, '0');
  return `${minutes}:${remainder}`;
}

export function GlobalPlanQuestionQueue() {
  const [queue, setQueue] = useState<PlanQuestion[]>([]);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');
  const [customOpen, setCustomOpen] = useState(false);
  const [customAnswer, setCustomAnswer] = useState('');
  const [remaining, setRemaining] = useState(0);
  const current = queue[0];

  const reload = useCallback(async () => {
    try {
      setQueue(sortQueue(await api.pendingPlanQuestions()));
    } catch { /* realtime/polling will retry */ }
  }, []);

  useEffect(() => { void reload(); }, [reload]);
  useEffect(() => {
    const timer = window.setInterval(() => void reload(), 1500);
    return () => window.clearInterval(timer);
  }, [reload]);

  const onRealtime = useCallback((event: TimelineEvent) => {
    if (event.type === 'plan.question_pending' || event.type === 'plan.question_resolved' || event.type === 'system.resync_required') {
      void reload();
    }
  }, [reload]);
  useRealtime(onRealtime);

  useEffect(() => {
    setBusy(false);
    setProblem('');
    setCustomOpen(false);
    setCustomAnswer('');
  }, [current?.id]);

  const deadline = current?.deadlineAtMs ?? 0;
  useEffect(() => {
    if (!current) { setRemaining(0); return; }
    const tick = () => setRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    tick();
    const timer = window.setInterval(tick, 250);
    return () => window.clearInterval(timer);
  }, [current, deadline]);

  const queueText = useMemo(() => tr('{count} planning question(s) waiting. They are shown one at a time so no answer is missed.', { count: queue.length }), [queue.length]);
  const expired = Boolean(current && remaining === 0);

  const answer = useCallback(async (value: { kind: 'option'; optionIndex: 1 | 2 } | { kind: 'custom'; text: string }) => {
    if (!current || busy || expired) return;
    setBusy(true);
    setProblem('');
    try {
      await api.answerPlanQuestion(current.id, value);
      setQueue((items) => items.filter((item) => item.id !== current.id));
    } catch (error) {
      if (error instanceof ApiError && (error.status === 404 || error.status === 409)) {
        await reload();
      } else {
        setProblem(error instanceof Error ? error.message : tr('Could not send the planning answer.'));
      }
    } finally {
      setBusy(false);
    }
  }, [busy, current, expired, reload]);

  const submitCustom = useCallback(() => {
    const text = customAnswer.trim();
    if (!text) {
      setProblem(tr('Enter your suggestion before sending.'));
      return;
    }
    void answer({ kind: 'custom', text });
  }, [answer, customAnswer]);

  if (!current) return null;

  return <Modal
    title={tr('AI needs more information')}
    description={tr('The AI is waiting for this answer inside the current planning turn.')}
    close={() => {}}
    dismissible={false}
    className="plan-question-modal"
  >
    <div className="plan-question-meta" aria-live="polite">
      <span><Sparkles />{tr('Planning mode')}</span>
      <span><Clock3 />{expired ? tr('AI is choosing automatically…') : tr('{time} remaining', { time: formatRemaining(remaining) })}</span>
    </div>

    <div className="plan-question-queue-note"><ListChecks /><span>{queueText}</span></div>

    <section className="plan-question-copy" aria-labelledby="plan-question-text">
      <span>{tr('Planning question')}</span>
      <strong id="plan-question-text">{current.question}</strong>
    </section>

    <div className="plan-question-options" aria-label={tr('Suggested answers')}>
      {current.options.map((option, index) => {
        const optionIndex = (index + 1) as 1 | 2;
        return <button key={optionIndex} type="button" disabled={busy || expired} onClick={() => void answer({ kind: 'option', optionIndex })}>
          <span>{optionIndex}</span><strong>{option}</strong>
        </button>;
      })}
    </div>

    <button className="plan-question-custom-toggle" type="button" disabled={busy || expired} aria-expanded={customOpen} onClick={() => setCustomOpen((value) => !value)}>
      <MessageSquareMore />{tr('Suggest another answer')}
    </button>

    {customOpen && <div className="plan-question-custom">
      <label htmlFor="plan-question-custom-answer">{tr('Your suggestion')}</label>
      <textarea
        id="plan-question-custom-answer"
        rows={3}
        maxLength={2000}
        value={customAnswer}
        disabled={busy || expired}
        placeholder={tr('Enter a different answer for the AI…')}
        onChange={(event) => { setCustomAnswer(event.target.value); setProblem(''); }}
      />
      <div><small>{customAnswer.length}/2000</small><button className="button primary" type="button" disabled={busy || expired || !customAnswer.trim()} onClick={submitCustom}>{tr('Send suggestion')}</button></div>
    </div>}

    {problem && <p className="plan-question-error" role="alert">{problem}</p>}
    {expired && <p className="plan-question-expired" role="status">{tr('The 120-second response window ended. The AI can now choose one of its two options and continue.')}</p>}
  </Modal>;
}
