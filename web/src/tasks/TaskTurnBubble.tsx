import { Ban, BookOpen, Check, CheckCheck, CheckCircle2, ChevronDown, CircleAlert, CircleStop, Clock3, FileCode2, FilePenLine, GitBranch, LoaderCircle, MessageSquareText, Search, TerminalSquare, Wrench } from 'lucide-react';
import { lazy, Suspense, useLayoutEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { api } from '../api';
import type { TaskTurn, TimelineEvent } from '../types';
import { StopActivityDialog } from './StopActivityDialog';
import {
  activityCodeView,
  activityCommand,
  activityDuration,
  activityLabel,
  activityOutput,
  buildProcessBlocks,
  duration,
  eventText,
  findFinalResponse,
  findUserMessage,
  formatClockTime,
  latestMessage,
  summarizeActivities,
  type ToolActivity,
} from './taskTimeline';

const TaskCodeViewer = lazy(async () => ({ default: (await import('../TaskCodeViewer')).TaskCodeViewer }));

export function TaskTurnBubble({ turn, now }: { turn: TaskTurn; now: string }) {
  const events = turn.events ?? [];
  const response = findFinalResponse(events);
  const userMessage = findUserMessage(events);
  const processEvents = events.filter((event) => event !== response?.event && event !== userMessage?.event);
  const blocks = buildProcessBlocks(processEvents);
  const activities = blocks.flatMap((block) => block.type === 'activities' ? block.activities : []);
  const status = turn.status ?? 'incomplete';
  const startedAt = turn.startedAtUtc ?? events[0]?.occurredAt ?? now;
  const finishedAt = turn.completedAtUtc ?? response?.event.occurredAt;
  const stateLabel = status === 'running' ? 'Đang xử lý…' : status === 'failed' ? 'Thất bại' : status === 'incomplete' ? 'Chưa hoàn tất' : 'Đã hoàn tất';
  const isThinking = status === 'running' && !response && !activities.some((activity) => activity.status === 'started' || activity.status === 'pending_approval');
  const headingId = `turn-${turn.id}`;
  const taskId = events.find((event) => event.taskId)?.taskId ?? '';
  const [stopTarget, setStopTarget] = useState<ToolActivity | null>(null);

  return <div className="turn-item">
    {userMessage && <article className="turn-user-message">
      <header><strong>Bạn</strong><time dateTime={userMessage.event.occurredAt}>{formatClockTime(userMessage.event.occurredAt)}</time></header>
      <div className="turn-user-content"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>{userMessage.text}</ReactMarkdown></div>
    </article>}
    <div className={`turn-end-status ${status}`} role={status === 'running' ? 'status' : undefined}>
      {status === 'running'
        ? <><LoaderCircle className="spin" /><span>Đang xử lý trong {duration(startedAt, now)}</span></>
        : finishedAt
          ? <><Clock3 /><span>{status === 'completed' ? `Hoàn thành lúc ${formatClockTime(finishedAt)} · ${duration(startedAt, finishedAt)}` : status === 'incomplete' ? `Không có tín hiệu mới từ ${formatClockTime(finishedAt)} · ${duration(startedAt, finishedAt)}` : `Kết thúc lúc ${formatClockTime(finishedAt)} · ${duration(startedAt, finishedAt)}`}</span></>
          : null}
    </div>
    <div className="turn-item-divider" aria-hidden="true" />
    <article className={`turn-bubble ${status}`} aria-labelledby={headingId} aria-busy={status === 'running'}>
      <header className="turn-header">
        <span className="turn-avatar" aria-hidden="true">{status === 'running' ? <LoaderCircle className="spin" /> : status === 'failed' || status === 'incomplete' ? <CircleAlert /> : <CheckCircle2 />}</span>
        <div><h3 id={headingId}>Codex Agent</h3><p>{status === 'running' ? `${activities.length.toLocaleString('vi')} hoạt động` : <><span>{stateLabel}</span> · {activities.length.toLocaleString('vi')} hoạt động</>}</p></div>
        <time dateTime={startedAt} aria-hidden="true">{status === 'running' ? duration(startedAt, now) : formatClockTime(startedAt)}</time>
      </header>
      {(activities.length > 0 || blocks.some((block) => block.type === 'progress')) && <TurnProcess blocks={blocks} now={now} taskId={taskId} onStop={setStopTarget} />}
      {isThinking && <div className="turn-thinking" role="status"><span>Đang suy nghĩ và chuẩn bị phản hồi…</span></div>}
      {status === 'failed' && <div className="turn-error" role="alert"><CircleAlert /><div><strong>Lượt agent thất bại</strong><p>{latestMessage(events) || 'Agent không thể hoàn tất lượt này. Xem hoạt động phía trên để kiểm tra nguyên nhân.'}</p></div></div>}
      {status === 'incomplete' && <div className="turn-warning" role="status"><CircleAlert /><div><strong>Có thể lượt đã bị gián đoạn</strong><p>{latestMessage(events) || 'Không nhận được hoạt động mới hoặc tín hiệu hoàn tất trong một thời gian dài. Có thể lượt bị gián đoạn hoặc tín hiệu đến muộn; trạng thái sẽ tự phục hồi khi có dữ liệu mới.'}</p></div></div>}
      {status === 'completed' && response && <div className="turn-response"><div className="turn-response-label"><CheckCircle2 /> Phản hồi cuối</div><div className="turn-response-content"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]} components={{ a: ({ node: _node, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener" /> }}>{response.text}</ReactMarkdown></div></div>}
    </article>
    {stopTarget && taskId && <StopActivityDialog taskId={taskId} activity={stopTarget} onClose={() => setStopTarget(null)} />}
  </div>;
}

function TurnProcess({ blocks, now, taskId, onStop }: { blocks: ReturnType<typeof buildProcessBlocks>; now: string; taskId: string; onStop: (activity: ToolActivity) => void }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const nearBottomRef = useRef(true);
  const last = blocks.at(-1);
  const updateKey = `${blocks.length}:${last?.key ?? 'empty'}:${last?.type === 'activities' ? last.activities.at(-1)?.finishedAt ?? '' : last?.event.occurredAt ?? ''}`;
  useLayoutEffect(() => {
    if (!nearBottomRef.current) return;
    const root = scrollRef.current;
    if (!root) return;
    const frame = window.requestAnimationFrame(() => root.scrollTo({ top: root.scrollHeight, behavior: 'auto' }));
    return () => window.cancelAnimationFrame(frame);
  }, [updateKey]);
  const updateScrollPosition = () => {
    const root = scrollRef.current;
    if (root) nearBottomRef.current = root.scrollHeight - root.scrollTop - root.clientHeight < 48;
  };
  return <div ref={scrollRef} className="turn-activities turn-process" role="region" tabIndex={0} aria-label="Tiến trình của agent" onScroll={updateScrollPosition}>
    {blocks.map((block) => block.type === 'progress'
      ? <ProgressMessage event={block.event} key={block.key} />
      : <section className="turn-activity-batch" key={block.key} aria-label={summarizeActivities(block.activities)}><div className="turn-activity-summary" role="status"><Wrench aria-hidden="true" /><p>{summarizeActivities(block.activities)}</p></div><div className="turn-activity-rows">{block.activities.map((activity) => <ActivityRow activity={activity} now={now} taskId={taskId} onStop={onStop} key={activity.id} />)}</div></section>)}
  </div>;
}

function ProgressMessage({ event }: { event: TimelineEvent }) {
  return <div className="turn-progress-message"><MessageSquareText aria-hidden="true" /><p>{eventText(event)}</p><time dateTime={event.occurredAt}>{formatClockTime(event.occurredAt)}</time></div>;
}

function ActivityRow({ activity, now, taskId, onStop }: { activity: ToolActivity; now: string; taskId: string; onStop: (activity: ToolActivity) => void }) {
  const approvalPending = activity.status === 'pending_approval';
  const stopRequested = activity.status === 'stop_requested';
  const stopped = activity.status === 'stopped';
  const running = activity.status === 'started' || approvalPending || stopRequested;
  const failed = activity.status === 'failed';
  const stoppable = activity.status === 'started';
  const Icon = stopped ? CircleStop : failed ? CircleAlert : activity.kind === 'read' ? BookOpen : activity.kind === 'search' ? Search : ['edit', 'create', 'delete', 'copy', 'move'].includes(activity.kind) ? FilePenLine : activity.kind === 'git' ? GitBranch : activity.kind === 'tool' ? Wrench : TerminalSquare;
  const command = activityCommand(activity);
  const output = activityOutput(activity);
  const codeView = activityCodeView(activity);
  return <div className={`terminal-activity ${running ? 'running' : ''} ${stopRequested ? 'stopping' : ''} ${stopped ? 'stopped' : ''} ${failed ? 'failed' : ''}`}>
    <details open={approvalPending || undefined}><summary><span className="activity-row-icon" aria-hidden="true">{running ? <LoaderCircle className="spin" /> : <Icon />}</span><span className="activity-label">{activityLabel(activity)}</span><span className="activity-timing"><time dateTime={activity.startedAt}>{formatClockTime(activity.startedAt)}</time><span aria-label={`Thời gian thực thi ${activityDuration(activity.startedAt, activity.finishedAt ?? now)}`}>· {activityDuration(activity.startedAt, activity.finishedAt ?? now)}</span></span><ChevronDown className="activity-chevron" aria-hidden="true" /></summary>
      <div className="activity-detail"><div className="activity-command"><FileCode2 /><code>{command}</code></div>{codeView
        ? <Suspense fallback={<pre tabIndex={0} aria-label={`Output của ${command}`}><code>{codeView.code}</code></pre>}><TaskCodeViewer {...codeView} label={`Output của ${command}`} /></Suspense>
        : <pre tabIndex={0} aria-label={`Output của ${command}`}><code>{output || (approvalPending ? 'Đang chờ bạn phê duyệt…' : running ? 'Đang chờ output…' : 'Lệnh không có output.')}</code></pre>}</div>
    </details>
    {stoppable && <button type="button" className="activity-stop-button" aria-label={`Dừng ${activityLabel(activity)}`} onClick={(event) => { event.preventDefault(); event.stopPropagation(); onStop(activity); }}><CircleStop aria-hidden="true" /><span>Dừng</span></button>}
    {approvalPending && taskId && <ApprovalActions taskId={taskId} activity={activity} />}
  </div>;
}

function ApprovalActions({ taskId, activity }: { taskId: string; activity: ToolActivity }) {
  const [busy, setBusy] = useState<'allow' | 'allowSimilar' | 'reject' | null>(null);
  const [error, setError] = useState('');
  const resolve = async (decision: 'allow' | 'allowSimilar' | 'reject') => {
    if (busy) return;
    setBusy(decision);
    setError('');
    try {
      await api.resolveTaskApproval(taskId, activity.id, { turnId: activity.turnId, decision });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Không thể gửi quyết định phê duyệt.');
      setBusy(null);
    }
  };
  return <div className="approval-actions" role="group" aria-label="Quyết định quyền chạy lệnh">
    <button type="button" className="approval-allow" disabled={Boolean(busy)} onClick={() => void resolve('allow')}>{busy === 'allow' ? <LoaderCircle className="spin" /> : <Check />}Cho phép</button>
    <button type="button" className="approval-similar" disabled={Boolean(busy)} onClick={() => void resolve('allowSimilar')}>{busy === 'allowSimilar' ? <LoaderCircle className="spin" /> : <CheckCheck />}Cho phép tương tự</button>
    <button type="button" className="approval-reject" disabled={Boolean(busy)} onClick={() => void resolve('reject')}>{busy === 'reject' ? <LoaderCircle className="spin" /> : <Ban />}Từ chối</button>
    {error && <p role="alert"><CircleAlert />{error}</p>}
  </div>;
}
