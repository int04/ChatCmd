import { BookOpen, Bot, CheckCircle2, ChevronDown, CircleAlert, CircleStop, Clock3, ExternalLink, FileCode2, FilePenLine, GitBranch, LoaderCircle, MessageSquareText, Search, TerminalSquare, Wrench } from 'lucide-react';
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { Modal } from '../components';
import { appLocale, formatAppNumber, tr } from '../i18n';
import type { SubagentRun, TaskTurn, TimelineEvent } from '../types';
import { ApprovalDecisionActions } from './ApprovalDecisionActions';
import { StopActivityDialog } from './StopActivityDialog';
import {
  activityCodeView,
  activityDiffView,
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
  fsSearchCodeViews,
  latestMessage,
  summarizeActivities,
  type ToolActivity,
} from './taskTimeline';

const TaskCodeViewer = lazy(async () => ({ default: (await import('../TaskCodeViewer')).TaskCodeViewer }));

export function TaskTurnBubble({ turn, taskId, subagents = [], agentLabel = 'Codex Agent' }: { turn: TaskTurn; taskId: string; subagents?: SubagentRun[]; agentLabel?: string }) {
  const events = turn.events ?? [];
  const response = findFinalResponse(events);
  const userMessage = findUserMessage(events);
  const visibleUserMessage = userMessage?.text.replace(/^\s*CMDGPT_SUBAGENT_ID=subagent-[A-Za-z0-9_-]+\s*$/gm, '').trim();
  const processEvents = events.filter((event) => event !== response?.event && event !== userMessage?.event);
  const blocks = buildProcessBlocks(processEvents);
  const activities = blocks.flatMap((block) => block.type === 'activities' ? block.activities : []);
  const rawStatus = turn.status ?? 'incomplete';
  const status = response ? 'completed' : rawStatus;
  const startedAt = turn.startedAtUtc ?? events[0]?.occurredAt ?? new Date().toISOString();
  const finishedAt = turn.completedAtUtc ?? response?.event.occurredAt;
  const stateLabel = status === 'running' ? tr('Processing…') : status === 'failed' ? tr('Failed') : status === 'incomplete' ? tr('Incomplete') : tr('Completed');
  const isThinking = status === 'running' && !response && !activities.some((activity) => activity.status === 'started' || activity.status === 'pending_approval');
  const headingId = `turn-${turn.id}`;
  const [stopTarget, setStopTarget] = useState<ToolActivity | null>(null);
  const [changeTarget, setChangeTarget] = useState<ToolActivity | null>(null);
  const fileChanges = response ? responseFileChanges(response.event) : [];

  return <div className="turn-item">
    {userMessage && <article className="turn-user-message">
      <header><strong>{tr('You')}</strong><BubbleTime value={userMessage.event.occurredAt} /></header>
      <div className="turn-user-content"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>{visibleUserMessage}</ReactMarkdown></div>
    </article>}
    <div className={`turn-end-status ${status}`} role={status === 'running' ? 'status' : undefined}>
      {status === 'running'
        ? <><LoaderCircle className="spin" /><span>{tr('Running for {duration}', { duration: '' }).replace(/\s+$/, '')} <LiveDuration startedAt={startedAt} /></span></>
        : finishedAt
          ? <><Clock3 /><span>{status === 'completed'
            ? tr('Completed {time} · {duration}', { time: bubbleTimePhraseText(finishedAt), duration: duration(startedAt, finishedAt) })
            : status === 'incomplete'
              ? tr('No new signal since {time} · {duration}', { time: bubbleTimePhraseText(finishedAt), duration: duration(startedAt, finishedAt) })
              : tr('Ended {time} · {duration}', { time: bubbleTimePhraseText(finishedAt), duration: duration(startedAt, finishedAt) })}</span></>
          : null}
    </div>
    <div className="turn-item-divider" aria-hidden="true" />
    <article className={`turn-bubble ${status}`} aria-labelledby={headingId} aria-busy={status === 'running'}>
      <header className="turn-header">
        <span className="turn-avatar" aria-hidden="true">{status === 'running' ? <LoaderCircle className="spin" /> : status === 'failed' || status === 'incomplete' ? <CircleAlert /> : <CheckCircle2 />}</span>
        <div><h3 id={headingId}>{agentLabel}</h3><p>{status === 'running' ? tr('{count} activities', { count: formatAppNumber(activities.length) }) : <><span>{stateLabel}</span> · {tr('{count} activities', { count: formatAppNumber(activities.length) })}</>}</p></div>
        {status === 'running' ? <time dateTime={startedAt} aria-hidden="true"><LiveDuration startedAt={startedAt} /></time> : <BubbleTime value={startedAt} ariaHidden />}
      </header>
      {subagents.length > 0 && <SubagentList agents={subagents} />}
      {(activities.length > 0 || blocks.some((block) => block.type === 'progress')) && <TurnProcess blocks={blocks} taskId={taskId} onStop={setStopTarget} />}
      {isThinking && <div className="turn-thinking" role="status"><span>{tr('Thinking and preparing a response…')}</span></div>}
      {status === 'failed' && (agentLabel === 'ChatGPT' && isChatGptSendDisabledMessage(latestMessage(events))
        ? <div className="turn-warning" role="status"><CircleAlert /><div><strong>{tr('Waiting to retry')}</strong><p>{tr('The ChatGPT send button is temporarily disabled. The system will retry in 10 seconds; you can cancel the send below.')}</p></div></div>
        : <div className="turn-error" role="alert"><CircleAlert /><div><strong>{tr('Agent turn failed')}</strong><p>{latestMessage(events) || tr('The Agent could not complete this turn. Review the activity above to find the cause.')}</p></div></div>)}
      {status === 'incomplete' && <div className="turn-warning" role="status"><CircleAlert /><div><strong>{tr('This turn may have been interrupted')}</strong><p>{latestMessage(events) || tr('No new activity or completion signal was received for a long time. The turn may have been interrupted or delayed; its state will recover automatically if new data arrives.')}</p></div></div>}
      {status === 'completed' && response && <div className="turn-response"><div className="turn-response-label"><CheckCircle2 /> {tr('Final response')}</div><div className="turn-response-content"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]} components={{ a: ({ node: _node, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener" /> }}>{response.text}</ReactMarkdown></div></div>}
      {status === 'completed' && fileChanges.length > 0 && <TurnFileChanges changes={fileChanges} onOpen={(activity) => setChangeTarget(activity)} />}
    </article>
    {stopTarget && taskId && <StopActivityDialog taskId={taskId} activity={stopTarget} onClose={() => setStopTarget(null)} />}
    {changeTarget && <ActivityDiffModal activity={changeTarget} close={() => setChangeTarget(null)} />}
  </div>;
}

type TurnFileChange = { path: string; fileName: string; extension: string; kind: 'added' | 'deleted' | 'modified'; additions: number; deletions: number; activity: ToolActivity };

function responseFileChanges(event: TimelineEvent): TurnFileChange[] {
  const payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {};
  const raw = Array.isArray(payload.fileChanges) ? payload.fileChanges : [];
  return raw.flatMap((item, index) => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) return [];
    const value = item as Record<string, unknown>;
    const path = typeof value.path === 'string' ? value.path : '';
    const before = typeof value.before === 'string' ? value.before : '';
    const after = typeof value.after === 'string' ? value.after : '';
    const kind = value.kind === 'added' || value.kind === 'deleted' ? value.kind : 'modified';
    if (!path) return [];
    const fileName = path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
    const extension = fileName.includes('.') ? fileName.split('.').at(-1)?.toUpperCase() || 'FILE' : 'FILE';
    const tool = kind === 'deleted' ? 'fs_delete' : 'fs_write_text';
    const activity: ToolActivity = {
      id: `file-change-${index}-${path}`,
      tool,
      kind: kind === 'deleted' ? 'delete' : 'edit',
      input: { path },
      output: { __chatcmdDiff: { path, before, after, beforeAvailable: value.beforeAvailable !== false } },
      status: 'succeeded',
      startedAt: event.occurredAt,
      finishedAt: event.occurredAt,
      turnId: event.turnId,
    };
    return [{ path, fileName, extension, kind, additions: Number(value.additions) || 0, deletions: Number(value.deletions) || 0, activity }];
  });
}

function TurnFileChanges({ changes, onOpen }: { changes: TurnFileChange[]; onOpen: (activity: ToolActivity) => void }) {
  return <section className="turn-file-changes" aria-label={tr('Changed files')}>
    <div className="turn-file-changes-heading"><FilePenLine aria-hidden="true" /><strong>{tr('Changed files')}</strong><span>{changes.length}</span></div>
    <div className="turn-file-change-list">{changes.map((change, index) => {
      const action = change.kind === 'added' ? tr('Added') : change.kind === 'deleted' ? tr('Deleted') : tr('Modified');
      return <button type="button" className={`turn-file-change-card ${change.kind}`} onClick={() => onOpen(change.activity)} key={`${change.path}:${index}`}>
        <span className="turn-file-change-icon"><FileCode2 aria-hidden="true" /><small>{change.extension}</small></span>
        <span className="turn-file-change-copy"><strong>{action} {change.fileName}</strong><small>{change.path}</small></span>
        <span className="turn-file-change-lines"><b>+{change.additions}</b><i>-{change.deletions}</i></span>
      </button>;
    })}</div>
  </section>;
}

function ActivityDiffModal({ activity, close }: { activity: ToolActivity; close: () => void }) {
  const diffView = activityDiffView(activity);
  if (!diffView) return null;
  const command = activityCommand(activity);
  return <Modal className="tool-activity-modal" title={activityLabel(activity)} description={`${formatClockTime(activity.startedAt)} · ${activityDuration(activity.startedAt, activity.finishedAt ?? new Date().toISOString())}`} close={close}>
    <div className="activity-popup-content">
      <div className="activity-command"><FileCode2 /><code>{command}</code></div>
      <div className="tool-diff-view"><div className="tool-diff-pane"><div className="tool-diff-tab removed">{tr('Original file')}</div><code className="tool-diff-path">{diffView.path}</code>{diffView.beforeAvailable ? <Suspense fallback={<pre><code>{diffView.before}</code></pre>}><TaskCodeViewer code={diffView.before} path={diffView.path} highlightedLines={diffView.beforeMarks} label={tr('Original file')} /></Suspense> : <div className="tool-diff-unavailable">{tr('The original content was not available for this shell change.')}</div>}</div><div className="tool-diff-pane"><div className="tool-diff-tab added">{tr('Modified file')}</div><code className="tool-diff-path">{diffView.path}</code><Suspense fallback={<pre><code>{diffView.after}</code></pre>}><TaskCodeViewer code={diffView.after} path={diffView.path} highlightedLines={diffView.afterMarks} label={tr('Modified file')} /></Suspense></div></div>
    </div>
  </Modal>;
}

function SubagentList({ agents }: { agents: SubagentRun[] }) {
  return <section className="turn-subagents" aria-label={tr('Subagents')}>
    <div className="turn-subagents-heading"><Bot aria-hidden="true" /><strong>{tr('Subagents')}</strong><span>{agents.length}</span></div>
    <div className="turn-subagents-list">{agents.map((agent) => <SubagentItem agent={agent} key={agent.id} />)}</div>
  </section>;
}

function SubagentItem({ agent }: { agent: SubagentRun }) {
  const pending = agent.status === 'pending';
  const running = agent.status === 'running';
  const failed = agent.status === 'failed';
  const stopped = agent.status === 'stopped';
  const statusLabel = pending ? tr('Waiting to start') : running ? tr('Running') : agent.status === 'completed' ? tr('Done') : agent.status === 'stopped' ? tr('Stopped') : agent.status === 'interrupted' ? tr('Interrupted') : failed ? tr('Failed') : agent.status;
  const content = <>
    <span className={`turn-subagent-state ${agent.status}`} aria-hidden="true">{pending ? <Clock3 /> : running ? <LoaderCircle className="spin" /> : stopped ? <CircleStop /> : failed || agent.status === 'interrupted' ? <CircleAlert /> : <CheckCircle2 />}</span>
    <span className="turn-subagent-copy"><strong>{agent.name}</strong><small>{statusLabel}</small></span>
    {agent.taskId && <ExternalLink className="turn-subagent-open" aria-hidden="true" />}
  </>;
  return agent.taskId
    ? <a className={`turn-subagent ${agent.status}`} href={`/tasks/${encodeURIComponent(agent.taskId)}`} target="_blank" rel="noreferrer noopener" aria-label={`${agent.name} - ${statusLabel} - ${tr('Open in new tab')}`}>{content}</a>
    : <div className={`turn-subagent ${agent.status}`} aria-label={`${agent.name} - ${statusLabel}`}>{content}</div>;
}

function TurnProcess({ blocks, taskId, onStop }: { blocks: ReturnType<typeof buildProcessBlocks>; taskId: string; onStop: (activity: ToolActivity) => void }) {
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
  const updateScrollPosition = () => { const root = scrollRef.current; if (root) nearBottomRef.current = root.scrollHeight - root.scrollTop - root.clientHeight < 48; };
  return <div ref={scrollRef} className="turn-activities turn-process" role="region" tabIndex={0} aria-label={tr('Agent progress')} onScroll={updateScrollPosition}>
    {blocks.map((block) => block.type === 'progress'
      ? <ProgressMessage event={block.event} key={block.key} />
      : <section className="turn-activity-batch" key={block.key} aria-label={summarizeActivities(block.activities)}><div className="turn-activity-summary" role="status"><Wrench aria-hidden="true" /><p>{summarizeActivities(block.activities)}</p></div><div className="turn-activity-rows">{block.activities.map((activity) => <ActivityRow activity={activity} taskId={taskId} onStop={onStop} key={activity.id} />)}</div></section>)}
  </div>;
}

function ProgressMessage({ event }: { event: TimelineEvent }) {
  return <div className="turn-progress-message"><MessageSquareText aria-hidden="true" /><p>{eventText(event)}</p><time dateTime={event.occurredAt}>{formatClockTime(event.occurredAt)}</time></div>;
}

function ActivityRow({ activity, taskId, onStop }: { activity: ToolActivity; taskId: string; onStop: (activity: ToolActivity) => void }) {
  const [open, setOpen] = useState(false);
  const [recentlyViewed, setRecentlyViewed] = useState(false);
  useEffect(() => { if (!recentlyViewed) return; const timer = window.setTimeout(() => setRecentlyViewed(false), 3000); return () => window.clearTimeout(timer); }, [recentlyViewed]);
  const closePopup = () => { setOpen(false); setRecentlyViewed(false); window.requestAnimationFrame(() => setRecentlyViewed(true)); };
  const approvalPending = activity.status === 'pending_approval';
  const stopRequested = activity.status === 'stop_requested';
  const stopped = activity.status === 'stopped';
  const running = activity.status === 'started' || approvalPending || stopRequested;
  const failed = activity.status === 'failed';
  const stoppable = activity.status === 'started';
  const Icon = stopped ? CircleStop : failed ? CircleAlert : activity.kind === 'read' ? BookOpen : activity.kind === 'search' ? Search : ['edit', 'create', 'delete', 'copy', 'move'].includes(activity.kind) ? FilePenLine : activity.kind === 'git' ? GitBranch : activity.kind === 'tool' ? Wrench : TerminalSquare;
  const command = activityCommand(activity);
  const output = activityOutput(activity);
  const searchCodeViews = fsSearchCodeViews(activity);
  const diffView = activityDiffView(activity);
  const codeView = activityCodeView(activity);
  return <div className={`terminal-activity ${running ? 'running' : ''} ${stopRequested ? 'stopping' : ''} ${stopped ? 'stopped' : ''} ${failed ? 'failed' : ''} ${recentlyViewed ? 'recently-viewed' : ''}`}>
    <button type="button" className="activity-popup-trigger" onClick={() => setOpen(true)} aria-haspopup="dialog">
      <span className="activity-row-icon" aria-hidden="true">{running ? <LoaderCircle className="spin" /> : <Icon />}</span>
      <span className="activity-label">{activityLabel(activity)}</span>
      <span className="activity-timing"><BubbleTime value={activity.startedAt} /><span aria-label={tr('Execution time')}>· {running ? <LiveActivityDuration startedAt={activity.startedAt} /> : activityDuration(activity.startedAt, activity.finishedAt ?? activity.startedAt)}</span></span>
      <ChevronDown className="activity-chevron" aria-hidden="true" />
    </button>
    {stoppable && <button type="button" className="activity-stop-button" aria-label={tr('Stop {name}', { name: activityLabel(activity) })} onClick={(event) => { event.preventDefault(); event.stopPropagation(); onStop(activity); }}><CircleStop aria-hidden="true" /><span>{tr('Stop')}</span></button>}
    {approvalPending && taskId && <ApprovalDecisionActions target={{ taskId, activityId: activity.id, turnId: activity.turnId }} />}
    {open && <Modal className="tool-activity-modal" title={activityLabel(activity)} description={`${formatClockTime(activity.startedAt)} · ${activityDuration(activity.startedAt, activity.finishedAt ?? new Date().toISOString())}`} close={closePopup}>
      <div className="activity-popup-content">
        <div className="activity-command"><FileCode2 /><code>{command}</code></div>
        {failed && <div className="activity-error-detail" role="alert">
          <div className="activity-error-heading"><CircleAlert aria-hidden="true" /><strong>{tr('Tool failed')}</strong></div>
          {activity.errorCode && <div className="activity-error-row"><span>{tr('Error code')}</span><code>{activity.errorCode}</code></div>}
          <div className="activity-error-message">{activity.errorMessage || activity.error || tr('Tool returned failed status without an error message.')}</div>
          {activity.errorDetails !== undefined && activity.errorDetails !== null && <pre tabIndex={0} aria-label={tr('Tool error details')}><code>{formatErrorDetails(activity.errorDetails)}</code></pre>}
        </div>}
        {diffView
          ? <div className="tool-diff-view"><div className="tool-diff-pane"><div className="tool-diff-tab removed">{tr('Original file')}</div><code className="tool-diff-path">{diffView.path}</code><Suspense fallback={<pre><code>{diffView.before}</code></pre>}><TaskCodeViewer code={diffView.before} path={diffView.path} highlightedLines={diffView.beforeMarks} label={tr('Original file')} /></Suspense></div><div className="tool-diff-pane"><div className="tool-diff-tab added">{tr('Modified file')}</div><code className="tool-diff-path">{diffView.path}</code><Suspense fallback={<pre><code>{diffView.after}</code></pre>}><TaskCodeViewer code={diffView.after} path={diffView.path} highlightedLines={diffView.afterMarks} label={tr('Modified file')} /></Suspense></div></div>
          : searchCodeViews.length > 0
          ? <div className="fs-search-code-results">{searchCodeViews.map((view, index) => <div className="fs-search-code-result" key={`${view.path}:${view.startLine}:${index}`}><code className="fs-search-result-path">{view.path}</code><Suspense fallback={<pre tabIndex={0} aria-label={tr('Output of {command}', { command })}><code>{view.code}</code></pre>}><TaskCodeViewer {...view} label={view.path} /></Suspense></div>)}</div>
          : codeView
            ? <Suspense fallback={<pre tabIndex={0} aria-label={tr('Output of {command}', { command })}><code>{codeView.code}</code></pre>}><TaskCodeViewer {...codeView} label={tr('Output of {command}', { command })} /></Suspense>
            : <pre tabIndex={0} aria-label={tr('Output of {command}', { command })}><code>{output || (approvalPending ? tr('Waiting for your approval…') : running ? tr('Waiting for output…') : tr('Command produced no output.'))}</code></pre>}
      </div>
    </Modal>}
  </div>;
}

function formatErrorDetails(value: unknown) { if (typeof value === 'string') return value; try { return JSON.stringify(value, null, 2); } catch { return String(value); } }
function isChatGptSendDisabledMessage(value: string) { return value.includes('Nút gửi ChatGPT đang bị vô hiệu hóa.') || value.includes('The ChatGPT send button is disabled.'); }

function BubbleTime({ value, ariaHidden = false }: { value: string; ariaHidden?: boolean }) {
  const nowMs = useAdaptiveNow(value);
  return <time dateTime={value} title={bubbleTimeHint(value)} aria-hidden={ariaHidden || undefined}>{bubbleTimeLabel(value, nowMs)}</time>;
}

function bubbleTimePhraseText(value: string) {
  const timestamp = Date.parse(value);
  const nowMs = Date.now();
  const elapsedSeconds = Number.isFinite(timestamp) ? Math.max(0, Math.floor((nowMs - timestamp) / 1000)) : 0;
  const label = bubbleTimeLabel(value, nowMs);
  return elapsedSeconds >= 3600 ? tr('at {time}', { time: label }) : label;
}

function LiveDuration({ startedAt }: { startedAt: string }) { const nowMs = useLiveSecondClock(); return <>{duration(startedAt, new Date(nowMs).toISOString())}</>; }
function LiveActivityDuration({ startedAt }: { startedAt: string }) { const nowMs = useLiveSecondClock(); return <>{activityDuration(startedAt, new Date(nowMs).toISOString())}</>; }

function useAdaptiveNow(value: string) {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const timestamp = Date.parse(value); if (!Number.isFinite(timestamp)) return; let timer: number | undefined;
    const schedule = () => { const current = Date.now(); const elapsed = Math.max(0, current - timestamp); if (elapsed >= 3_600_000) return; const interval = elapsed < 60_000 ? 1_000 : 60_000; const delay = interval - (current % interval) + 20; timer = window.setTimeout(() => { if (document.visibilityState !== 'visible') { timer = undefined; return; } setNowMs(Date.now()); schedule(); }, delay); };
    const onVisibility = () => { if (document.visibilityState !== 'visible') return; if (timer !== undefined) window.clearTimeout(timer); setNowMs(Date.now()); schedule(); };
    schedule(); document.addEventListener('visibilitychange', onVisibility); return () => { if (timer !== undefined) window.clearTimeout(timer); document.removeEventListener('visibilitychange', onVisibility); };
  }, [value]);
  return nowMs;
}

function useLiveSecondClock() {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    let timer: number | undefined;
    const schedule = () => { timer = window.setTimeout(() => { if (document.visibilityState !== 'visible') { timer = undefined; return; } setNowMs(Date.now()); schedule(); }, 1_000); };
    const onVisibility = () => { if (document.visibilityState !== 'visible') return; if (timer !== undefined) window.clearTimeout(timer); setNowMs(Date.now()); schedule(); };
    schedule(); document.addEventListener('visibilitychange', onVisibility); return () => { if (timer !== undefined) window.clearTimeout(timer); document.removeEventListener('visibilitychange', onVisibility); };
  }, []);
  return nowMs;
}

function bubbleTimeLabel(value: string, nowMs: number) {
  const timestamp = Date.parse(value); if (!Number.isFinite(timestamp)) return value;
  const elapsedSeconds = Math.max(0, Math.floor((nowMs - timestamp) / 1000));
  if (elapsedSeconds < 60) return tr('{count} seconds ago', { count: elapsedSeconds });
  if (elapsedSeconds < 3600) return tr('{count} minutes ago', { count: Math.floor(elapsedSeconds / 60) });
  return new Intl.DateTimeFormat(appLocale(), { hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date(timestamp));
}
function bubbleTimeHint(value: string) { const timestamp = Date.parse(value); if (!Number.isFinite(timestamp)) return value; return new Intl.DateTimeFormat(appLocale(), { hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date(timestamp)); }

