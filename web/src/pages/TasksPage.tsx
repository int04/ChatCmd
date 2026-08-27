import { CheckCircle2, CircleAlert, CircleStop, Clock3, LoaderCircle, MessageSquareText, Search, TerminalSquare } from 'lucide-react';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { Link, useParams } from 'react-router-dom';

import { api } from '../api';

import { Empty, ErrorState, Loading, ProblemBanner, StatusBadge, formatTime } from '../components';

import type { Task, TaskDetail, TimelineEvent } from '../types';

import { TaskTurnBubble } from '../tasks/TaskTurnBubble';

import { buildTaskTurns, mergeLiveDetail, upsertTaskEvent } from '../tasks/taskTimeline';

import { useRealtime } from '../realtime';

import { useLoad } from '../useLoad';



export function TasksPage() { return <TasksWorkspace />; }

export function TaskDetailPage() { return <TasksWorkspace />; }



function TasksWorkspace() {

  const { taskId } = useParams();

  const result = useLoad(api.tasks, []);

  const [query, setQuery] = useState('');

  const { reload: reloadTasks, setData: setTasks } = result;

  const [detailVersion, setDetailVersion] = useState(0);

  const [liveEvents, setLiveEvents] = useState<TimelineEvent[]>([]);

  const connectedOnce = useRef(false);

  useEffect(() => setLiveEvents([]), [taskId]);

  const handleRealtime = useCallback((event: TimelineEvent) => {

    if (event.type === 'system.connected') {

      if (connectedOnce.current) { void reloadTasks(); setDetailVersion((value) => value + 1); }

      connectedOnce.current = true;

      return;

    }

    if (!event.taskId) return;

    setTasks((current) => upsertTaskEvent(current, event));

    if (event.taskId === taskId) setLiveEvents((current) => current.some((item) => item.id === event.id) ? current : [...current, event]);

  }, [reloadTasks, setTasks, taskId]);

  const realtime = useRealtime(handleRealtime);

  const tasks = useMemo(() => [...(result.data ?? [])]

    .sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc))

    .filter((task) => `${conversationName(task)} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, result.data]);



  return <div className="tasks-workspace">

    <aside className="tasks-conversation-pane" aria-label="Công việc gần đây">

      <header className="tasks-conversation-header"><div><span className="eyebrow">TASKS</span><h1>Công việc gần đây</h1></div><small>{tasks.length}</small></header>

      <label className="tasks-conversation-search"><Search /><span className="sr-only">Tìm công việc</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Tìm công việc" /></label>

      <div className="tasks-conversation-list">

        {result.loading ? <Loading label="Loading tasks" /> : result.error ? <ErrorState message={result.error} retry={() => void result.reload()} /> : !tasks.length ? <Empty title="Chưa có công việc" body="Task từ Agent sẽ xuất hiện tại đây." /> : tasks.map((task) => <ConversationRow task={task} selected={task.id === taskId} key={task.id} />)}

      </div>

    </aside>

    <section className="tasks-detail-pane" aria-label="Nội dung công việc">

      {taskId ? <TaskConversationDetail taskId={taskId} refreshVersion={detailVersion} realtime={realtime} liveEvents={liveEvents} /> : <div className="tasks-select-empty"><MessageSquareText /><strong>Chọn một cuộc trò chuyện</strong><span>Nội dung xử lý của Agent, tool và kết luận sẽ hiển thị tại đây.</span></div>}

    </section>

  </div>;

}



function ConversationRow({ task, selected }: { task: Task; selected: boolean }) {

  const running = task.status === 'running';

  return <Link className={`tasks-conversation-row ${selected ? 'selected' : ''}`} aria-current={selected ? 'page' : undefined} to={`/tasks/${encodeURIComponent(task.id)}`}>

    <span className={`tasks-conversation-icon ${running ? 'running' : ''}`}>{running ? <LoaderCircle className="spin" /> : <TerminalSquare />}</span>

    <span className="tasks-conversation-copy"><strong>{conversationName(task)}</strong><small>{task.outputPreview || `${task.turnCount ?? 0} lượt Agent · ${task.status}`}</small></span>

    <span className="tasks-conversation-tail"><time>{formatTime(task.updatedAtUtc)}</time>{running ? <LoaderCircle className="spin" /> : <i className={`conversation-state ${task.status}`} />}</span>

  </Link>;

}



function TaskConversationDetail({ taskId, refreshVersion, realtime, liveEvents }: { taskId: string; refreshVersion: number; realtime: string; liveEvents: TimelineEvent[] }) {

  const result = useLoad(() => api.task(taskId), [taskId, refreshVersion]);

  const [problem, setProblem] = useState('');

  const [busy, setBusy] = useState(false);

  if (result.loading) return <Loading label="Loading task" />;

  if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;

  const detail = mergeLiveDetail(result.data, liveEvents);

  return <TaskDetailContent detail={detail} realtime={realtime} problem={problem} clearProblem={() => setProblem('')} stop={async () => {

    setBusy(true); setProblem('');

    try { result.setData(await api.taskAction(taskId, 'stop')); } catch (error) { setProblem(error instanceof Error ? error.message : 'Action failed'); } finally { setBusy(false); }

  }} busy={busy} />;

}



function TaskDetailContent({ detail, realtime, problem, clearProblem, stop, busy }: { detail: TaskDetail; realtime: string; problem: string; clearProblem: () => void; stop: () => Promise<void>; busy: boolean }) {

  const { task, events = [] } = detail;

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

  return <div className="task-detail-shell">

    <header className="task-detail-topbar"><div><h1>{conversationName(task)}</h1><p>{turns.length} agent turns · generation {task.generation ?? 1} · {realtime === 'online' ? 'realtime' : realtime} · updated {formatTime(task.updatedAtUtc)}</p></div><StatusBadge state={task.status} /></header>

    <ProblemBanner message={problem} clear={clearProblem} />

    <div className="task-detail-body">

      <main className="task-chat-column"><h2 className="sr-only">Activity timeline</h2><section className="task-bubble-timeline turn-timeline" aria-label="Conversation activity">

        {turns.length ? turns.map((turn) => <TaskTurnBubble turn={turn} now={turnNow} key={turn.id} />) : <Empty title="Chưa có hoạt động" body="Agent chưa ghi nhận nội dung cho task này." />}

      </section></main>

      <aside className="task-detail-sidebar" aria-label="Task information">

        <header className="task-info-header"><span className={`task-info-state ${task.status}`}>{task.status === 'running' ? <LoaderCircle className="spin" /> : task.status === 'failed' ? <CircleAlert /> : <CheckCircle2 />}</span><div><h2>{conversationName(task)}</h2><p><code>#{task.id}</code> · {task.status} · {turns.length} lượt agent</p></div></header>

        <div className="task-info-duration"><Clock3 /><span>{formatTime(startedAt)} → {formatTime(task.updatedAtUtc)}</span></div>

        <section className="task-info-section"><strong>Terminal / Task</strong><div className="task-info-generation"><TerminalSquare /><div><code>Generation {task.generation ?? 1}</code><small>{task.activeSessionId ? `#${task.activeSessionId}` : 'No active terminal'}</small></div></div></section>

        <section className="task-info-section"><strong>Execution mode</strong><p>{detail.executionMode ?? 'approval'}</p></section>

        <section className="task-info-section task-stop-card"><strong>Conversation</strong><p>Dừng cuộc trò chuyện và hoạt động hiện tại của Agent.</p><button className="button danger" disabled={busy || task.status !== 'running'} onClick={() => void stop()}><CircleStop />Stop conversation</button></section>

      </aside>

    </div>

  </div>;

}



function conversationName(task: Task) { return task.title?.trim() || generatedConversationName(task.id); }

function generatedConversationName(id: string) { const first = ['Mây', 'Sao', 'Gió', 'Nắng', 'Trăng', 'Biển', 'Rừng', 'Sương']; const second = ['Xanh', 'Nhẹ', 'Sớm', 'Đêm', 'Mới', 'Xa', 'Êm', 'Sáng']; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
