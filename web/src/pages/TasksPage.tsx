import { CheckCircle2, CircleAlert, Clock3, LoaderCircle, MessageSquareText, PanelRightClose, PanelRightOpen } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useParams } from 'react-router-dom';

import { api } from '../api';
import { ChatGptTaskCard, ChatGptTaskComposer, NewChatGptConversation } from '../chatgpt/ChatGptConversation';
import { ErrorState, Loading, StatusBadge, formatTime } from '../components';
import { tr, translatedStatus } from '../i18n';
import { useRealtime } from '../realtime';
import { TaskAccessCard } from '../tasks/TaskAccessCard';
import { TaskConversationStopCard } from '../tasks/TaskConversationStopCard';
import { SubagentApprovalQueue } from '../tasks/SubagentApprovalQueue';
import { TaskTerminalSection } from '../tasks/TaskTerminalSection';
import { TaskTurnBubble } from '../tasks/TaskTurnBubble';
import { useResizableWidth } from '../tasks/useResizableWidth';
import { buildTaskTurns, mergeLiveDetail } from '../tasks/taskTimeline';
import type { Task, TaskDetail, TimelineEvent } from '../types';
import { useLoad } from '../useLoad';

export function TasksPage() { return <TasksWorkspace />; }

function TasksWorkspace() {
  const { taskId } = useParams();
  const [detailVersion, setDetailVersion] = useState(0);
  const [liveEvents, setLiveEvents] = useState<TimelineEvent[]>([]);
  const connectedOnce = useRef(false);
  const hiddenSubagentTaskIds = useRef(new Set<string>());
  useEffect(() => setLiveEvents([]), [taskId]);
  const hideSubagentTask = useCallback((childTaskId: string) => { hiddenSubagentTaskIds.current.add(childTaskId); }, []);
  const handleRealtime = useCallback((event: TimelineEvent) => {
    if (event.type === 'system.connected') { if (connectedOnce.current) setDetailVersion((value) => value + 1); connectedOnce.current = true; return; }
    if (event.type === 'system.resync_required') { setDetailVersion((value) => value + 1); return; }
    if (event.type === 'subagent.status' || event.type === 'subagent.approval_pending' || event.type === 'subagent.approval_resolved') {
      const childTaskId = subagentChildTaskId(event); if (childTaskId) hideSubagentTask(childTaskId); if (event.taskId === taskId) setDetailVersion((value) => value + 1); return;
    }
    if (!event.taskId) return;
    if (event.taskId === taskId && event.type === 'approval.resolved') {
      const payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {};
      const activityId = typeof payload.activityId === 'string' ? payload.activityId : '';
      const decision = typeof payload.decision === 'string' ? payload.decision : '';
      if (activityId) {
        setLiveEvents((current) => [...current, {
          id: `approval-resolved:${event.id}`,
          type: 'tool_call',
          taskId: event.taskId,
          turnId: event.turnId,
          occurredAt: event.occurredAt,
          payload: { activityId, status: decision === 'reject' ? 'rejected' : 'approved' },
        }]);
      }
      setDetailVersion((value) => value + 1);
      return;
    }
    if (event.taskId === taskId && (event.type === 'approval.pending' || event.type === 'conversation.approval_pending' || event.type === 'conversation.title_updated')) { setDetailVersion((value) => value + 1); return; }
    if (event.taskId === taskId) {
      const compactEvent = compactLiveToolEvent(event);
      setLiveEvents((current) => current.some((item) => item.id === compactEvent.id) ? current : [...current, compactEvent]);
    }
    else if (taskId && hiddenSubagentTaskIds.current.has(event.taskId)) setDetailVersion((value) => value + 1);
  }, [hideSubagentTask, taskId]);
  const realtime = useRealtime(handleRealtime);

  return <div className="tasks-workspace"><section className="tasks-detail-pane" aria-label={tr('Task content')}>
    {taskId === 'new' ? <NewChatGptConversation /> : taskId ? <TaskConversationDetail taskId={taskId} refreshVersion={detailVersion} realtime={realtime} liveEvents={liveEvents} onSubagentTask={hideSubagentTask} /> : <div className="tasks-select-empty"><MessageSquareText /><strong>{tr('Choose a conversation')}</strong><span>{tr('Agent processing, tools, and conclusions will appear here.')}</span></div>}
  </section></div>;
}

function TaskConversationDetail({ taskId, refreshVersion, realtime, liveEvents, onSubagentTask }: { taskId: string; refreshVersion: number; realtime: string; liveEvents: TimelineEvent[]; onSubagentTask: (taskId: string) => void }) {
  const result = useLoad(() => api.task(taskId), [taskId]);
  const refresh = result.refresh;
  const [olderEvents, setOlderEvents] = useState<TimelineEvent[]>([]);
  const [nextCursor, setNextCursor] = useState<string>();
  const [loadingOlder, setLoadingOlder] = useState(false);
  useEffect(() => { setOlderEvents([]); setNextCursor(undefined); setLoadingOlder(false); }, [taskId]);
  useEffect(() => { if (!olderEvents.length) setNextCursor(result.data?.nextCursor); }, [olderEvents.length, result.data?.nextCursor]);
  useEffect(() => { if (refreshVersion > 0) void refresh(); }, [refreshVersion, refresh]);
  useEffect(() => { if (result.data?.task.isSubagent) onSubagentTask(result.data.task.id); for (const subagent of result.data?.subagents ?? []) if (subagent.taskId) onSubagentTask(subagent.taskId); }, [onSubagentTask, result.data?.subagents, result.data?.task.id, result.data?.task.isSubagent]);
  const approvalPolling = result.data?.task.allowExecute === null || (result.data?.executionMode === 'approval' && (result.data.task.status === 'running' || (result.data.subagents ?? []).some((subagent) => subagent.status === 'pending' || subagent.status === 'running')));
  useEffect(() => { if (!approvalPolling) return; const timer = window.setInterval(() => void refresh(), 1500); return () => window.clearInterval(timer); }, [approvalPolling, refresh]);
  const loadOlder = useCallback(async () => {
    if (!nextCursor || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await api.task(taskId, nextCursor);
      setOlderEvents((current) => mergeUniqueEvents(page.events ?? [], current));
      setNextCursor(page.nextCursor);
    } finally { setLoadingOlder(false); }
  }, [loadingOlder, nextCursor, taskId]);
  if (result.loading) return <Loading label={tr('Loading task')} />;
  if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;
  const baseDetail = { ...result.data, events: mergeUniqueEvents(olderEvents, result.data.events ?? []) };
  const detail = mergeLiveDetail(baseDetail, liveEvents);
  return <TaskDetailContent detail={detail} realtime={realtime} onTaskChanged={result.setData} hasOlder={Boolean(nextCursor)} loadingOlder={loadingOlder} onLoadOlder={loadOlder} />;
}

function TaskDetailContent({ detail, realtime, onTaskChanged, hasOlder, loadingOlder, onLoadOlder }: { detail: TaskDetail; realtime: string; onTaskChanged: (detail: TaskDetail) => void; hasOlder: boolean; loadingOlder: boolean; onLoadOlder: () => Promise<void> }) {
  const { task, events = [] } = detail;
  const chatGpt = task.source === 'chatgpt_web';
  const turns = useMemo(() => detail.turns?.length ? detail.turns : buildTaskTurns(events, task), [detail.turns, events, task]);
  const startedAt = task.createdAtUtc ?? turns[0]?.startedAtUtc ?? task.updatedAtUtc;
  const chatRef = useRef<HTMLElement>(null);
  const nearBottomRef = useRef(true);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem('chatcmd.layout.taskDetailSidebarCollapsed.v1') === 'true');
  const sidebarResize = useResizableWidth({ storageKey: 'chatcmd.layout.taskDetailSidebarWidth.v1', cssVariable: '--task-detail-sidebar-width', defaultWidth: typeof window !== 'undefined' && window.innerWidth <= 1180 ? 300 : 340, minWidth: 280, maxWidth: 520, direction: -1 });
  const activeTaskRef = useRef<string | null>(null);
  const lastEvent = events.at(-1); const lastTurn = turns.at(-1);
  const updateKey = `${events.length}:${lastEvent?.id ?? 'empty'}:${turns.length}:${lastTurn?.id ?? 'empty'}:${lastTurn?.status ?? 'empty'}:${lastTurn?.completedAtUtc ?? ''}`;
  useLayoutEffect(() => { const root = chatRef.current; if (!root) return; const taskChanged = activeTaskRef.current !== task.id; activeTaskRef.current = task.id; if (!taskChanged && !nearBottomRef.current) return; const frame = window.requestAnimationFrame(() => { root.scrollTop = root.scrollHeight; }); return () => window.cancelAnimationFrame(frame); }, [task.id, updateKey]);
  const toggleSidebar = () => setSidebarCollapsed((current) => {
    const next = !current;
    localStorage.setItem('chatcmd.layout.taskDetailSidebarCollapsed.v1', String(next));
    return next;
  });
  const updateChatScrollPosition = () => {
    const root = chatRef.current; if (!root) return;
    nearBottomRef.current = root.scrollHeight - root.scrollTop - root.clientHeight < 96;
    if (root.scrollTop > 40 || !hasOlder || loadingOlder) return;
    const previousHeight = root.scrollHeight;
    void onLoadOlder().then(() => window.requestAnimationFrame(() => {
      const current = chatRef.current;
      if (current) current.scrollTop += current.scrollHeight - previousHeight;
    }));
  };

  return <div className={`task-detail-shell${sidebarCollapsed ? ' sidebar-collapsed' : ''}`}>
    <header className="task-detail-topbar"><div><h1>{conversationName(task)}</h1><p>{tr('{count} agent turns · generation {generation} · {realtime} · updated {time}', { count: turns.length, generation: task.generation ?? 1, realtime: realtime === 'online' ? translatedStatus('online') : realtime, time: formatTime(task.updatedAtUtc) })}</p></div><div className="task-detail-topbar-actions"><StatusBadge state={task.status} /><button className="task-detail-sidebar-toggle" type="button" aria-label={sidebarCollapsed ? 'Mở thông tin task' : 'Đóng thông tin task'} title={sidebarCollapsed ? 'Mở thông tin task' : 'Đóng thông tin task'} onClick={toggleSidebar}>{sidebarCollapsed ? <PanelRightOpen /> : <PanelRightClose />}</button></div></header>
    <div className="task-detail-body">
      <div className={`task-chat-pane${chatGpt ? ' has-chatgpt-footer' : ''}`}>
        <main ref={chatRef} className="task-chat-column" onScroll={updateChatScrollPosition}><h2 className="sr-only">{tr('Activity timeline')}</h2>
          <SubagentApprovalQueue approvals={detail.subagentApprovals ?? []} onResolved={(activityId) => onTaskChanged({ ...detail, subagentApprovals: (detail.subagentApprovals ?? []).filter((item) => item.activityId !== activityId) })} />
          {loadingOlder && <div className="task-history-skeleton" role="status" aria-label={tr('Loading older conversation')}><span /><span /><span /></div>}
          <section className="task-bubble-timeline turn-timeline" aria-label={tr('Conversation activity')}>
            {turns.length ? turns.map((turn) => <TaskTurnBubble turn={turn} taskId={task.id} agentLabel={chatGpt ? 'ChatGPT' : tr('Codex Agent')} subagents={(detail.subagents ?? []).filter((agent) => agent.parentTurnId === turn.id)} key={turn.id} />) : <div className="task-awaiting-first-response" role="status" aria-live="polite"><span className="task-awaiting-first-response-icon"><LoaderCircle /></span><span>{tr('The conversation is connected, ChatGPT is thinking about the answer...')}</span></div>}
          </section>
        </main>
        {chatGpt && <footer className="task-chat-footer"><ChatGptTaskComposer taskId={task.id} /></footer>}
      </div>
      {!sidebarCollapsed && <aside className="task-detail-sidebar" aria-label={tr('Task information')}>
        <div className="panel-resize-handle task-sidebar-resize-handle" role="separator" aria-label={tr('Resize task information')} aria-orientation="vertical" aria-valuemin={280} aria-valuemax={520} aria-valuenow={sidebarResize.width} tabIndex={0} onPointerDown={sidebarResize.onPointerDown} onKeyDown={sidebarResize.onKeyDown} />
        <header className="task-info-header"><span className={`task-info-state ${task.status}`}>{task.status === 'running' ? <LoaderCircle className="spin" /> : task.status === 'failed' ? <CircleAlert /> : <CheckCircle2 />}</span><div><h2>{conversationName(task)}</h2><p><code>#{task.id}</code> · {translatedStatus(task.status)} · {tr('{count} agent turns', { count: turns.length })}</p></div></header>
        <div className="task-info-duration"><Clock3 /><span>{formatTime(startedAt)} → {formatTime(task.updatedAtUtc)}</span></div>
        <TaskTerminalSection taskId={task.id} turnId={lastTurn?.id} />
        {(chatGpt || task.isSubagent) && <ChatGptTaskCard taskId={task.id} />}
        <TaskAccessCard taskId={detail.executionModeSourceTaskId ?? task.id} grantTaskId={task.id} defaultMode={detail.executionMode ?? 'allowAll'} grants={detail.approvalGrants} />
        {!chatGpt && <TaskConversationStopCard taskId={task.id} taskStatus={task.status} onStopped={onTaskChanged} />}
      </aside>}
    </div>
  </div>;
}


function mergeUniqueEvents(first: TimelineEvent[], second: TimelineEvent[]) {
  const merged = new Map<string, TimelineEvent>();
  for (const event of [...first, ...second]) if (!merged.has(event.id)) merged.set(event.id, event);
  return [...merged.values()].sort((left, right) => left.occurredAt.localeCompare(right.occurredAt) || left.id.localeCompare(right.id));
}

function compactLiveToolEvent(event: TimelineEvent): TimelineEvent {
  if ((event.type !== 'tool_call' && event.type !== 'tool_result') || !event.payload || typeof event.payload !== 'object' || Array.isArray(event.payload)) return event;
  const payload = { ...(event.payload as Record<string, unknown>) };
  if (event.type === 'tool_call' && payload.input && typeof payload.input === 'object' && !Array.isArray(payload.input)) {
    const source = payload.input as Record<string, unknown>;
    const input: Record<string, unknown> = {};
    for (const key of ['path', 'workingDirectory', 'query', 'command', 'source', 'destination', 'name', 'pattern']) if (source[key] !== undefined) input[key] = source[key];
    payload.input = input;
  } else if (event.type === 'tool_result') {
    delete payload.output;
    delete payload.errorDetails;
    delete payload.details;
    if (typeof payload.error !== 'string') delete payload.error;
  }
  return { ...event, payload };
}
function subagentChildTaskId(event: TimelineEvent) { if (!event.payload || typeof event.payload !== 'object' || Array.isArray(event.payload)) return ''; const value = (event.payload as Record<string, unknown>).childTaskId; return typeof value === 'string' ? value.trim() : ''; }
function conversationName(task: Task) { return task.title?.trim() || task.agentName?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = [tr('Cloud'), tr('Star'), tr('Wind'), tr('Sun'), tr('Moon'), tr('Sea'), tr('Forest'), tr('Mist')]; const second = [tr('Blue'), tr('Soft'), tr('Morning'), tr('Night'), tr('New'), tr('Far'), tr('Warm'), tr('Bright')]; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
