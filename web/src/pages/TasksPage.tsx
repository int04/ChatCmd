import { Ban, BookOpen, CheckCircle2, ChevronDown, CircleAlert, CircleStop, Clock3, FileCode2, FilePenLine, GitBranch, LoaderCircle, MessageSquareText, Search, TerminalSquare, Wrench } from 'lucide-react';
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api';
import { Empty, ErrorState, Loading, ProblemBanner, StatusBadge, formatTime } from '../components';
import type { Task, TaskDetail, TaskTurn, TimelineEvent } from '../types';
import { useRealtime } from '../realtime';
import { useLoad } from '../useLoad';

const TaskCodeViewer = lazy(async () => ({ default: (await import('../TaskCodeViewer')).TaskCodeViewer }));

type ToolKind = 'read' | 'search' | 'edit' | 'git' | 'command' | 'tool';
type ToolActivity = { id: string; tool: string; kind: ToolKind; input?: unknown; output?: unknown; status: string; error?: string; startedAt: string; finishedAt?: string };
type ProcessBlock = { kind: 'message'; key: string; event: TimelineEvent } | { kind: 'tools'; key: string; activities: ToolActivity[] };

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
    setTasks((current) => current?.map((task) => task.id === event.taskId ? mergeTaskEvent(task, event) : task));
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
  const turns = useMemo(() => detail.turns?.length ? detail.turns : deriveTurns(events, task), [detail.turns, events, task]);
  const startedAt = task.createdAtUtc ?? turns[0]?.startedAtUtc ?? task.updatedAtUtc;
  return <div className="task-detail-shell">
    <header className="task-detail-topbar"><div><h1>{conversationName(task)}</h1><p>{turns.length} agent turns · generation {task.generation ?? 1} · {realtime === 'online' ? 'realtime' : realtime} · updated {formatTime(task.updatedAtUtc)}</p></div><StatusBadge state={task.status} /></header>
    <ProblemBanner message={problem} clear={clearProblem} />
    <div className="task-detail-body">
      <main className="task-chat-column"><h2 className="sr-only">Activity timeline</h2><section className="task-bubble-timeline" aria-label="Conversation activity">
        {turns.length ? turns.map((turn, index) => <TurnBubble turn={turn} index={index} key={turn.id} />) : <Empty title="Chưa có hoạt động" body="Agent chưa ghi nhận nội dung cho task này." />}
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

function TurnBubble({ turn, index }: { turn: TaskTurn; index: number }) {
  const events = turn.events ?? [];
  const response = findFinalResponse(events);
  const blocks = buildProcessBlocks(response ? events.filter((event) => event !== response.event) : events);
  const activities = blocks.flatMap((block) => block.kind === 'tools' ? block.activities : []);
  const running = turn.status === 'running';
  const failed = turn.status === 'failed';
  const incomplete = turn.status === 'incomplete';
  const started = turn.startedAtUtc ?? events[0]?.occurredAt;
  const ended = turn.completedAtUtc ?? response?.event.occurredAt;
  const stateLabel = running ? 'Đang xử lý' : failed ? 'Thất bại' : incomplete ? 'Chưa hoàn tất' : 'Hoàn thành';
  return <div className={`task-turn-item ${turn.status ?? 'unknown'}`}>
    <div className="task-turn-end-status">{running ? <><LoaderCircle className="spin" /><span>Đang xử lý</span></> : ended ? <><Clock3 /><span>Kết thúc {formatTime(ended)}</span></> : null}</div>
    <div className="task-turn-divider" aria-hidden="true" />
    <article className={`task-turn-bubble ${turn.status ?? 'unknown'}`}>
      <header className="task-turn-header"><span className="task-turn-avatar">{running ? <LoaderCircle className="spin" /> : failed || incomplete ? <CircleAlert /> : <CheckCircle2 />}</span><div><h3>Agent</h3><p><span>{stateLabel}</span> · {activities.length} hoạt động · Turn {index + 1}</p></div><time>{started ? formatTime(started) : ''}</time></header>
      {blocks.length > 0 && <div className="task-turn-process" aria-label="Quá trình xử lý">{blocks.map((block) => block.kind === 'message' ? <AgentMessage event={block.event} key={block.key} /> : <ToolBatch activities={block.activities} key={block.key} />)}</div>}
      {running && blocks.length === 0 && <div className="task-turn-thinking"><LoaderCircle className="spin" /><span>Agent đang suy nghĩ…</span></div>}
      {response && <div className="task-final-response"><div className="task-final-label"><CheckCircle2 /> Phản hồi cuối</div><div className="task-final-content agent-rich-content"><AgentRichText>{response.text}</AgentRichText></div></div>}
      {(failed || incomplete) && !response && <div className="task-turn-error"><CircleAlert /><span>{latestMessage(events) || (incomplete ? 'Lượt xử lý chưa có phản hồi cuối.' : 'Agent turn failed.')}</span></div>}
    </article>
  </div>;
}

function AgentMessage({ event }: { event: TimelineEvent }) {
  const text = eventText(event) || cleanEventType(event.type);
  return <div className="task-progress-message"><MessageSquareText /><div className="task-progress-content agent-rich-content"><AgentRichText>{text}</AgentRichText></div><time>{formatTime(event.occurredAt)}</time></div>;
}

function AgentRichText({ children }: { children: string }) {
  return <ReactMarkdown
    remarkPlugins={[remarkGfm]}
    rehypePlugins={[rehypeSanitize]}
    components={{ a: ({ node: _node, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener" /> }}
  >{children}</ReactMarkdown>;
}
function ToolBatch({ activities }: { activities: ToolActivity[] }) {
  return <section className="task-tool-batch"><div className="task-tool-summary"><Wrench /><p>{summarizeTools(activities)}</p></div><div className="task-tool-rows">{activities.map((activity) => <ToolActivityRow activity={activity} key={activity.id} />)}</div></section>;
}

function ToolActivityRow({ activity }: { activity: ToolActivity }) {
  const Icon = activity.kind === 'read' ? BookOpen : activity.kind === 'search' ? Search : activity.kind === 'edit' ? FilePenLine : activity.kind === 'git' ? GitBranch : activity.kind === 'command' ? TerminalSquare : Wrench;
  const target = activityTarget(activity);
  const command = activityCommand(activity);
  const output = activityOutput(activity);
  const path = activityPath(activity);
  const failed = activity.status === 'failed';
  const running = activity.status === 'started';
  return <div className={`task-tool-activity ${failed ? 'failed' : ''} ${running ? 'running' : ''}`}><details>
    <summary><span className="task-tool-icon">{running ? <LoaderCircle className="spin" /> : failed ? <CircleAlert /> : <Icon />}</span><span className="task-tool-title">{activityVerb(activity.kind)}{target ? `: ${target}` : ''}</span><span className="task-tool-time"><time>{formatTime(activity.startedAt)}</time>{activity.finishedAt && <small>→ {formatTime(activity.finishedAt)}</small>}</span><ChevronDown /></summary>
    <div className="task-tool-detail"><div className="task-tool-command"><FileCode2 /><code>{command}</code></div>{output ? <Suspense fallback={<pre className="task-code-fallback"><code>{output.text}</code></pre>}><TaskCodeViewer code={output.text} path={path} language={output.language} label={`${activity.tool} output`} /></Suspense> : <div className="task-tool-no-output">{running ? 'Đang chờ output…' : 'Không có output được ghi nhận.'}</div>}{activity.error && <div className="task-tool-error"><CircleAlert />{activity.error}</div>}</div>
  </details></div>;
}

function buildProcessBlocks(events: TimelineEvent[]): ProcessBlock[] {
  const blocks: ProcessBlock[] = [];
  const pending = new Map<string, ToolActivity>();
  const pendingByTool = new Map<string, ToolActivity[]>();
  let currentTools: ToolActivity[] | null = null;
  for (const event of events) {
    if (event.type === 'tool_call') {
      const payload = payloadObject(event);
      const tool = stringValue(payload.tool) || 'tool';
      if (isHousekeepingTool(tool)) continue;
      const activity: ToolActivity = { id: stringValue(payload.activityId) || event.id, tool, kind: toolKind(tool), input: payload.input, status: stringValue(payload.status) || 'started', startedAt: event.occurredAt };
      if (!currentTools) { currentTools = []; blocks.push({ kind: 'tools', key: `tools-${event.id}`, activities: currentTools }); }
      currentTools.push(activity);
      pending.set(activity.id, activity);
      const queue = pendingByTool.get(tool) ?? []; queue.push(activity); pendingByTool.set(tool, queue);
      continue;
    }
    if (event.type === 'tool_result') {
      const payload = payloadObject(event);
      const tool = stringValue(payload.tool) || 'tool';
      if (isHousekeepingTool(tool)) continue;
      const id = stringValue(payload.activityId);
      const queue = pendingByTool.get(tool) ?? [];
      const activity = (id && pending.get(id)) || queue.find((item) => !item.finishedAt);
      if (activity) { activity.output = payload.output; activity.status = stringValue(payload.status) || 'succeeded'; activity.finishedAt = event.occurredAt; activity.error = stringValue(payload.errorMessage) || stringValue(payload.errorCode); }
      else { currentTools ??= []; if (!blocks.length || blocks.at(-1)?.kind !== 'tools') blocks.push({ kind: 'tools', key: `tools-${event.id}`, activities: currentTools }); currentTools.push({ id: id || event.id, tool, kind: toolKind(tool), output: payload.output, status: stringValue(payload.status) || 'succeeded', error: stringValue(payload.errorMessage) || stringValue(payload.errorCode), startedAt: event.occurredAt, finishedAt: event.occurredAt }); }
      continue;
    }
    if (event.type === 'terminal_output' && currentTools?.length) { const latest = currentTools.at(-1)!; latest.output = appendOutput(latest.output, eventText(event)); continue; }
    if (isVisibleAgentMessage(event)) { currentTools = null; blocks.push({ kind: 'message', key: event.id, event }); }
  }
  return blocks.filter((block) => block.kind === 'message' || block.activities.length > 0);
}

function deriveTurns(events: TimelineEvent[], task: Task): TaskTurn[] {
  const groups = new Map<string, TaskTurn>();
  events.forEach((event, index) => { const id = event.turnId || `legacy-${event.sessionId || 'task'}`; const current = groups.get(id) ?? { id, generation: task.generation ?? 1, actor: 'agent', status: 'running', startedAtUtc: event.occurredAt, events: [] }; current.events!.push(event); if (findFinalResponse(current.events!)) { current.status = 'completed'; current.completedAtUtc = event.occurredAt; } groups.set(id, current); if (!event.turnId && index === 0) current.startedAtUtc = event.occurredAt; });
  const turns = [...groups.values()];
  turns.forEach((turn, index) => { if (turn.status === 'completed') return; if (index === turns.length - 1 && task.status === 'running') turn.status = 'running'; else if (task.status === 'failed') turn.status = 'failed'; else turn.status = 'incomplete'; });
  return turns;
}

function findFinalResponse(events: TimelineEvent[]) { for (let index = events.length - 1; index >= 0; index--) { const event = events[index]; const payload = payloadObject(event); if (event.type === 'status' && stringValue(payload.status) === 'completed') { const text = stringValue(payload.content) || stringValue(payload.response) || stringValue(payload.message); if (text) return { event, text }; } } return null; }
function isVisibleAgentMessage(event: TimelineEvent) { if (!['progress', 'message', 'warning', 'status'].includes(event.type)) return false; const payload = payloadObject(event); return !(event.type === 'status' && stringValue(payload.status) === 'completed') && Boolean(eventText(event)); }
function eventText(event?: TimelineEvent) { if (!event) return ''; if (typeof event.payload === 'string') return event.payload.trim(); const payload = payloadObject(event); for (const key of ['content', 'message', 'text', 'response', 'plainText', 'errorMessage', 'error']) { const value = stringValue(payload[key]); if (value) return value; } return ''; }
function latestMessage(events: TimelineEvent[]) { for (let index = events.length - 1; index >= 0; index--) { const text = eventText(events[index]); if (text) return text; } return ''; }
function payloadObject(event: TimelineEvent): Record<string, unknown> { return event.payload && typeof event.payload === 'object' ? event.payload as Record<string, unknown> : {}; }
function stringValue(value: unknown) { return typeof value === 'string' && value.trim() ? value.trim() : ''; }
function isHousekeepingTool(tool: string) { return tool === 'agent_progress' || tool === 'agent_turn_complete'; }
function toolKind(tool: string): ToolKind { if (/^(fs_read|fs_list|fs_stat|fs_find|fs_directory|view_|file_download)/i.test(tool)) return 'read'; if (/search|find/i.test(tool)) return 'search'; if (/write|edit|patch|copy|move|delete|create/i.test(tool)) return 'edit'; if (/^git_/i.test(tool)) return 'git'; if (/shell_|terminal|execute|command/i.test(tool)) return 'command'; return 'tool'; }
function activityVerb(kind: ToolKind) { return kind === 'read' ? 'Read' : kind === 'search' ? 'Search' : kind === 'edit' ? 'Edit' : kind === 'git' ? 'Git' : kind === 'command' ? 'Execute' : 'Tool'; }
function activityTarget(activity: ToolActivity) { const input = asObject(activity.input); for (const key of ['path', 'workingDirectory', 'query', 'command', 'source', 'destination', 'name']) { const value = stringValue(input[key]); if (value) return compact(value, 96); } return activity.tool; }
function activityCommand(activity: ToolActivity) { const input = asObject(activity.input); const command = stringValue(input.command); if (command) return command; const path = stringValue(input.path); if (path) return `${activity.tool}: ${path}`; const legacy = legacyTerminalParts(activity.output); if (legacy.command) return legacy.command; return activity.input === undefined ? activity.tool : `${activity.tool} ${formatValue(activity.input)}`; }
function activityPath(activity: ToolActivity) { const input = asObject(activity.input); const output = asObject(activity.output); return stringValue(output.path) || stringValue(input.path) || null; }
function activityOutput(activity: ToolActivity): { text: string; language?: string } | null { if (activity.output === undefined) return activity.error ? { text: activity.error, language: 'plain' } : null; if (typeof activity.output === 'string') return { text: stripAnsi(activity.output), language: 'plain' }; const value = asObject(activity.output); for (const key of ['code', 'content', 'text', 'plainText', 'output']) { const text = stringValue(value[key]); if (!text) continue; if (key === 'plainText' && activity.kind === 'command' && activity.input === undefined) { const legacy = legacyTerminalParts(activity.output); return { text: legacy.output || stripAnsi(text), language: 'plain' }; } return { text: key === 'code' ? text : stripAnsi(text), language: key === 'code' ? undefined : 'plain' }; } return { text: formatValue(activity.output), language: 'json' }; }
function legacyTerminalParts(output: unknown) { const value = asObject(output); const raw = stringValue(value.plainText) || stringValue(value.text) || (typeof output === 'string' ? output : ''); if (!raw) return { command: '', output: '' }; const lines = stripAnsi(raw).replace(/\r/g, '').split('\n').map((line) => line.replace(/\s+$/g, '')); const meaningful = lines.map((line, index) => ({ line, index })).filter(({ line }) => line.trim() && !isTerminalPrompt(line)); const first = meaningful[0]; if (!first) return { command: '', output: '' }; const command = first.line.trim(); const rest = lines.slice(first.index + 1).filter((line) => !isTerminalPrompt(line)).join('\n').trim(); return { command, output: rest }; }
function isTerminalPrompt(value: string) { const line = value.trim(); return !line || /^PS\s+.+?>\s*$/i.test(line) || /^>>\s*$/.test(line); }
function stripAnsi(value: string) { let result = ''; for (let index = 0; index < value.length;) { if (value.charCodeAt(index) !== 27) { result += value[index++]; continue; } index++; const marker = value[index]; if (marker === ']') { index++; while (index < value.length && value.charCodeAt(index) !== 7 && !(value.charCodeAt(index) === 27 && value[index + 1] === '\\')) index++; if (value.charCodeAt(index) === 27) index += 2; else if (value.charCodeAt(index) === 7) index++; continue; } if (marker === '[') { index++; while (index < value.length) { const code = value.charCodeAt(index++); if (code >= 64 && code <= 126) break; } continue; } index++; } return result; }
function appendOutput(output: unknown, text: string) { if (!text) return output; if (typeof output === 'string') return output + text; const value = asObject(output); const current = stringValue(value.plainText) || stringValue(value.text); return { ...value, plainText: current + text }; }
function asObject(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
function formatValue(value: unknown) { return typeof value === 'string' ? value : JSON.stringify(value ?? {}, null, 2); }
function summarizeTools(activities: ToolActivity[]) { const counts = new Map<ToolKind, number>(); activities.forEach((activity) => counts.set(activity.kind, (counts.get(activity.kind) ?? 0) + 1)); const names: Record<ToolKind, string> = { read: 'read', search: 'search', edit: 'edit', git: 'Git', command: 'command', tool: 'tool' }; return [...counts.entries()].map(([kind, count]) => `${count} ${names[kind]}`).join(' · '); }
function cleanEventType(type: string) { return type.replace(/[._-]+/g, ' ').replace(/\s+/g, ' ').trim(); }
function compact(value: string, max: number) { const text = value.replace(/\s+/g, ' ').trim(); return text.length > max ? `${text.slice(0, max - 1)}…` : text; }
function conversationName(task: Task) { return task.title?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = ['Mây', 'Sao', 'Gió', 'Nắng', 'Trăng', 'Biển', 'Rừng', 'Sương']; const second = ['Xanh', 'Nhẹ', 'Sớm', 'Đêm', 'Mới', 'Xa', 'Êm', 'Sáng']; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }


function mergeTaskEvent(task: Task, event: TimelineEvent): Task {
  const payload = asObject(event.payload);
  const status = stringValue(payload.status);
  const title = stringValue(payload.title);
  return {
    ...task,
    status: status === 'completed' ? 'completed' : event.type === 'progress' || event.type === 'tool_call' ? 'running' : task.status,
    title: title || task.title,
    updatedAtUtc: event.occurredAt || task.updatedAtUtc,
    outputPreview: stringValue(payload.content) || task.outputPreview,
  };
}

function mergeLiveDetail(detail: TaskDetail, liveEvents: TimelineEvent[]): TaskDetail {
  if (!liveEvents.length) return detail;
  const events = [...(detail.events ?? [])];
  const seen = new Set(events.map((event) => event.id));
  for (const event of liveEvents) if (!seen.has(event.id)) { events.push(event); seen.add(event.id); }
  events.sort((left, right) => Date.parse(left.occurredAt) - Date.parse(right.occurredAt) || left.id.localeCompare(right.id));
  const task = liveEvents.reduce(mergeTaskEvent, detail.task);
  return { ...detail, task, turns: undefined, events };
}
