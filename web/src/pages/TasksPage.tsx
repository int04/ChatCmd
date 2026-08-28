import { CheckCircle2, CircleAlert, Clock3, LoaderCircle, MessageSquareText, Plus, Search, TerminalSquare } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams } from 'react-router-dom';

import { api } from '../api';
import { ChatGptTaskCard, ChatGptTaskComposer, NewChatGptConversation } from '../chatgpt/ChatGptConversation';
import { Empty, ErrorState, Loading, StatusBadge, formatTime } from '../components';
import { useRealtime } from '../realtime';
import { TaskAccessCard } from '../tasks/TaskAccessCard';
import { TaskConversationStopCard } from '../tasks/TaskConversationStopCard';
import { SubagentApprovalQueue } from '../tasks/SubagentApprovalQueue';
import { TaskTurnBubble } from '../tasks/TaskTurnBubble';
import { buildTaskTurns, mergeLiveDetail, upsertTaskEvent } from '../tasks/taskTimeline';
import type { Task, TaskDetail, TimelineEvent } from '../types';
import { useLoad } from '../useLoad';

const READ_FINAL_COUNTS_KEY = 'chatcmd.tasks.readFinalCounts.v1';

export function TasksPage() { return <TasksWorkspace />; }

function TasksWorkspace() {
  const { taskId } = useParams();
  const result = useLoad(api.tasks, []);
  const [query, setQuery] = useState('');
  const { reload: reloadTasks, setData: setTasks } = result;
  const [detailVersion, setDetailVersion] = useState(0);
  const [liveEvents, setLiveEvents] = useState<TimelineEvent[]>([]);
  const [readFinalCounts, setReadFinalCounts] = useState<Record<string, number>>(readStoredFinalCounts);
  const connectedOnce = useRef(false);
  const hiddenSubagentTaskIds = useRef(new Set<string>());
  const visibleTaskIds = useRef(new Set<string>());
  const hadStoredReadCounts = useRef(typeof localStorage !== 'undefined' && localStorage.getItem(READ_FINAL_COUNTS_KEY) !== null);

  useEffect(() => setLiveEvents([]), [taskId]);
  useEffect(() => { visibleTaskIds.current = new Set((result.data ?? []).map((task) => task.id)); }, [result.data]);

  const hideSubagentTask = useCallback((childTaskId: string) => {
    hiddenSubagentTaskIds.current.add(childTaskId);
    setTasks((current) => current?.filter((task) => task.id !== childTaskId));
  }, [setTasks]);

  const handleRealtime = useCallback((event: TimelineEvent) => {
    if (event.type === 'system.connected') {
      if (connectedOnce.current) { void reloadTasks(); setDetailVersion((value) => value + 1); }
      connectedOnce.current = true;
      return;
    }
    if (event.type === 'subagent.status' || event.type === 'subagent.approval_pending' || event.type === 'subagent.approval_resolved') {
      const childTaskId = subagentChildTaskId(event);
      if (childTaskId) hideSubagentTask(childTaskId);
      if (event.taskId === taskId) setDetailVersion((value) => value + 1);
      return;
    }
    if (!event.taskId) return;
    if (!hiddenSubagentTaskIds.current.has(event.taskId)) {
      if (visibleTaskIds.current.has(event.taskId)) setTasks((current) => upsertTaskEvent(current, event));
      else void reloadTasks();
    }
    if (event.taskId === taskId) {
      setLiveEvents((current) => current.some((item) => item.id === event.id) ? current : [...current, event]);
    } else if (taskId && hiddenSubagentTaskIds.current.has(event.taskId)) {
      setDetailVersion((value) => value + 1);
    }
  }, [hideSubagentTask, reloadTasks, setTasks, taskId]);
  const realtime = useRealtime(handleRealtime);

  useEffect(() => {
    const next = result.data;
    if (!next || hadStoredReadCounts.current) return;
    hadStoredReadCounts.current = true;
    setReadFinalCounts(Object.fromEntries(next.map((task) => [task.id, task.finalResponseCount ?? 0])));
  }, [result.data]);

  useEffect(() => {
    try { localStorage.setItem(READ_FINAL_COUNTS_KEY, JSON.stringify(readFinalCounts)); } catch { /* storage can be unavailable */ }
  }, [readFinalCounts]);

  useEffect(() => {
    if (!taskId || !result.data) return;
    const task = result.data.find((item) => item.id === taskId);
    if (!task) return;
    const count = task.finalResponseCount ?? 0;
    setReadFinalCounts((current) => (current[taskId] ?? 0) >= count ? current : { ...current, [taskId]: count });
  }, [result.data, taskId]);

  const tasks = useMemo(() => [...(result.data ?? [])]
    .sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc))
    .filter((task) => `${conversationName(task)} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, result.data]);

  return <div className="tasks-workspace">
    <aside className="tasks-conversation-pane" aria-label="Công việc gần đây">
      <header className="tasks-conversation-header"><div><span className="eyebrow">TASKS</span><h1>Công việc gần đây</h1></div><div className="tasks-header-actions"><small>{tasks.length}</small><Link className="tasks-new-message" to="/tasks/new"><Plus /><span>Tin nhắn mới</span></Link></div></header>
      <label className="tasks-conversation-search"><Search /><span className="sr-only">Tìm công việc</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Tìm công việc" /></label>
      <div className="tasks-conversation-list">
        {result.loading ? <Loading label="Loading tasks" /> : result.error ? <ErrorState message={result.error} retry={() => void result.reload()} /> : !tasks.length ? <Empty title="Chưa có công việc" body="Task từ Agent sẽ xuất hiện tại đây." /> : tasks.map((task) => <ConversationRow task={task} selected={task.id === taskId} unread={Math.max(0, (task.finalResponseCount ?? 0) - (readFinalCounts[task.id] ?? 0))} key={task.id} />)}
      </div>
    </aside>
    <section className="tasks-detail-pane" aria-label="Nội dung công việc">
      {taskId === 'new' ? <NewChatGptConversation /> : taskId ? <TaskConversationDetail taskId={taskId} refreshVersion={detailVersion} realtime={realtime} liveEvents={liveEvents} onSubagentTask={hideSubagentTask} /> : <div className="tasks-select-empty"><MessageSquareText /><strong>Chọn một cuộc trò chuyện</strong><span>Nội dung xử lý của Agent, tool và kết luận sẽ hiển thị tại đây.</span></div>}
    </section>
  </div>;
}

function ConversationRow({ task, selected, unread }: { task: Task; selected: boolean; unread: number }) {
  const running = task.status === 'running';
  return <Link className={`tasks-conversation-row ${selected ? 'selected' : ''}`} aria-current={selected ? 'page' : undefined} to={`/tasks/${encodeURIComponent(task.id)}`}>
    <span className={`tasks-conversation-icon ${running ? 'running' : ''}`}>{running ? <LoaderCircle className="spin" /> : <TerminalSquare />}</span>
    <span className="tasks-conversation-copy"><strong>{conversationName(task)}</strong><small>{task.outputPreview || `${task.turnCount ?? 0} lượt Agent · ${task.status}`}</small></span>
    {unread > 0 && <span className="task-unread-badge" aria-label={`${unread} phản hồi mới chưa đọc`}>{unread > 99 ? '99+' : unread}</span>}
    <span className="tasks-conversation-tail"><time>{formatTime(task.updatedAtUtc)}</time>{running ? <LoaderCircle className="spin" /> : <i className={`conversation-state ${task.status}`} />}</span>
  </Link>;
}

function TaskConversationDetail({ taskId, refreshVersion, realtime, liveEvents, onSubagentTask }: { taskId: string; refreshVersion: number; realtime: string; liveEvents: TimelineEvent[]; onSubagentTask: (taskId: string) => void }) {
  const result = useLoad(() => api.task(taskId), [taskId]);
  useEffect(() => {
    if (refreshVersion > 0) void result.refresh();
  }, [refreshVersion, result.refresh]);
  useEffect(() => {
    if (result.data?.task.isSubagent) onSubagentTask(result.data.task.id);
    for (const subagent of result.data?.subagents ?? []) {
      if (subagent.taskId) onSubagentTask(subagent.taskId);
    }
  }, [onSubagentTask, result.data?.subagents, result.data?.task.id, result.data?.task.isSubagent]);
  const approvalPolling = result.data?.executionMode === 'approval' && (result.data.task.status === 'running' || (result.data.subagents ?? []).some((subagent) => subagent.status === 'pending' || subagent.status === 'running'));
  useEffect(() => {
    if (!approvalPolling) return;
    const timer = window.setInterval(() => void result.refresh(), 1500);
    return () => window.clearInterval(timer);
  }, [approvalPolling, result.refresh]);
  if (result.loading) return <Loading label="Loading task" />;
  if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;
  const detail = mergeLiveDetail(result.data, liveEvents);
  return <TaskDetailContent detail={detail} realtime={realtime} onTaskChanged={result.setData} />;
}

function TaskDetailContent({ detail, realtime, onTaskChanged }: { detail: TaskDetail; realtime: string; onTaskChanged: (detail: TaskDetail) => void }) {
  const { task, events = [] } = detail;
  const chatGpt = task.source === 'chatgpt_web';
  const turns = useMemo(() => detail.turns?.length ? detail.turns : buildTaskTurns(events, task), [detail.turns, events, task]);
  const [now, setNow] = useState(0);
  useEffect(() => {
    if (task.status !== 'running') return;
    setNow(Date.now());
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') setNow(Date.now()); }, 1000);
    return () => window.clearInterval(timer);
  }, [task.status]);
  const turnNow = task.status === 'running' && now > 0 ? new Date(now).toISOString() : turns.at(-1)?.completedAtUtc ?? task.updatedAtUtc;
  const startedAt = task.createdAtUtc ?? turns[0]?.startedAtUtc ?? task.updatedAtUtc;
  const chatRef = useRef<HTMLElement>(null);
  const nearBottomRef = useRef(true);
  const activeTaskRef = useRef<string | null>(null);
  const lastEvent = events.at(-1);
  const lastTurn = turns.at(-1);
  const updateKey = `${events.length}:${lastEvent?.id ?? 'empty'}:${turns.length}:${lastTurn?.id ?? 'empty'}:${lastTurn?.status ?? 'empty'}:${lastTurn?.completedAtUtc ?? ''}`;

  useLayoutEffect(() => {
    const root = chatRef.current;
    if (!root) return;
    const taskChanged = activeTaskRef.current !== task.id;
    activeTaskRef.current = task.id;
    if (!taskChanged && !nearBottomRef.current) return;
    const frame = window.requestAnimationFrame(() => { root.scrollTop = root.scrollHeight; });
    return () => window.cancelAnimationFrame(frame);
  }, [task.id, updateKey]);

  const updateChatScrollPosition = () => {
    const root = chatRef.current;
    if (root) nearBottomRef.current = root.scrollHeight - root.scrollTop - root.clientHeight < 96;
  };

  return <div className="task-detail-shell">
    <header className="task-detail-topbar"><div><h1>{conversationName(task)}</h1><p>{turns.length} agent turns · generation {task.generation ?? 1} · {realtime === 'online' ? 'realtime' : realtime} · updated {formatTime(task.updatedAtUtc)}</p></div><StatusBadge state={task.status} /></header>
    <div className="task-detail-body">
      <main ref={chatRef} className="task-chat-column" onScroll={updateChatScrollPosition}><h2 className="sr-only">Activity timeline</h2>
        <SubagentApprovalQueue approvals={detail.subagentApprovals ?? []} onResolved={(activityId) => onTaskChanged({ ...detail, subagentApprovals: (detail.subagentApprovals ?? []).filter((item) => item.activityId !== activityId) })} />
        <section className="task-bubble-timeline turn-timeline" aria-label="Conversation activity">
        {turns.length ? turns.map((turn) => <TaskTurnBubble turn={turn} now={turnNow} taskId={task.id} agentLabel={chatGpt ? 'ChatGPT' : 'Codex Agent'} subagents={(detail.subagents ?? []).filter((agent) => agent.parentTurnId === turn.id)} key={turn.id} />) : <Empty title="Chưa có hoạt động" body="Agent chưa ghi nhận nội dung cho task này." />}
      </section>{chatGpt && <ChatGptTaskComposer taskId={task.id} />}</main>
      <aside className="task-detail-sidebar" aria-label="Task information">
        <header className="task-info-header"><span className={`task-info-state ${task.status}`}>{task.status === 'running' ? <LoaderCircle className="spin" /> : task.status === 'failed' ? <CircleAlert /> : <CheckCircle2 />}</span><div><h2>{conversationName(task)}</h2><p><code>#{task.id}</code> · {task.status} · {turns.length} lượt agent</p></div></header>
        <div className="task-info-duration"><Clock3 /><span>{formatTime(startedAt)} → {formatTime(task.updatedAtUtc)}</span></div>
        <section className="task-info-section"><strong>Terminal / Task</strong><div className="task-info-generation"><TerminalSquare /><div><code>Generation {task.generation ?? 1}</code><small>{task.activeSessionId ? `#${task.activeSessionId}` : 'No active terminal'}</small></div></div></section>
        {chatGpt && <ChatGptTaskCard taskId={task.id} />}
        <TaskAccessCard taskId={detail.executionModeSourceTaskId ?? task.id} defaultMode={detail.executionMode ?? 'allowAll'} />
        {!chatGpt && <TaskConversationStopCard taskId={task.id} taskStatus={task.status} onStopped={onTaskChanged} />}
      </aside>
    </div>
  </div>;
}

function readStoredFinalCounts(): Record<string, number> {
  try {
    const value = JSON.parse(localStorage.getItem(READ_FINAL_COUNTS_KEY) ?? '{}') as Record<string, unknown>;
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] >= 0));
  } catch { return {}; }
}

function subagentChildTaskId(event: TimelineEvent) {
  if (!event.payload || typeof event.payload !== 'object' || Array.isArray(event.payload)) return '';
  const value = (event.payload as Record<string, unknown>).childTaskId;
  return typeof value === 'string' ? value.trim() : '';
}

function conversationName(task: Task) { return task.agentName?.trim() || task.title?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = ['Mây', 'Sao', 'Gió', 'Nắng', 'Trăng', 'Biển', 'Rừng', 'Sương']; const second = ['Xanh', 'Nhẹ', 'Sớm', 'Đêm', 'Mới', 'Xa', 'Ấm', 'Sáng']; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
