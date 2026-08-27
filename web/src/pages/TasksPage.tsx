import { Ban, Check, CircleStop, Filter, Search, TerminalSquare } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api';
import { Disclosure, Empty, ErrorState, Loading, PageHeading, ProblemBanner, StatusBadge, formatTime } from '../components';
import type { Task, TimelineEvent } from '../types';
import { useLoad } from '../useLoad';

export function TasksPage() {
  const result = useLoad(api.tasks, []); const [query, setQuery] = useState(''); const [status, setStatus] = useState('all');
  const tasks = useMemo(() => [...(result.data ?? [])].sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc)).filter((task) => status === 'all' || task.status === status).filter((task) => `${task.title ?? ''} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, result.data, status]);
  return <div><PageHeading eyebrow="WORK QUEUE" title="Tasks" body="Newest conversations, turns, tools, and approvals." />
    <div className="filter-bar"><label className="search-field"><Search /><span className="sr-only">Search tasks</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search title or ID" /></label><label className="select-field"><Filter /><span className="sr-only">Filter status</span><select value={status} onChange={(event) => setStatus(event.target.value)}><option value="all">All statuses</option><option value="running">Running</option><option value="completed">Completed</option><option value="failed">Failed</option><option value="stopped">Stopped</option></select></label></div>
    {result.loading ? <Loading label="Loading tasks" /> : result.error ? <ErrorState message={result.error} retry={() => void result.reload()} /> : !tasks.length ? <Empty title="No tasks found" body="Tasks created through local MCP appear here." /> : <div className="record-list">{tasks.map((task) => <TaskRow task={task} key={task.id} />)}</div>}
  </div>;
}
function TaskRow({ task }: { task: Task }) { return <Link className="record-row" to={`/tasks/${encodeURIComponent(task.id)}`}><span className="record-icon"><TerminalSquare /></span><div className="record-main"><strong>{task.title || `Task ${task.id}`}</strong><code>{task.id}</code><small>{task.outputPreview || `${task.turnCount ?? 0} turns`}</small></div><div className="record-meta"><StatusBadge state={task.status} />{task.approvalPending && <span className="approval-tag">Approval</span>}<time>{formatTime(task.updatedAtUtc)}</time></div></Link>; }

export function TaskDetailPage() {
  const { taskId = '' } = useParams(); const result = useLoad(() => api.task(taskId), [taskId]); const [problem, setProblem] = useState(''); const [busy, setBusy] = useState('');
  const action = async (name: string, body?: unknown) => { setBusy(name); setProblem(''); try { result.setData(await api.taskAction(taskId, name, body)); } catch (error) { setProblem(error instanceof Error ? error.message : 'Action failed'); } finally { setBusy(''); } };
  if (result.loading) return <Loading label="Loading task" />; if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;
  const { task, turns = [], events = [] } = result.data;
  return <div><PageHeading eyebrow={`TASK · ${task.id}`} title={task.title || `Task ${task.id}`} body={`${task.generation ?? 1} generations · ${task.turnCount ?? turns.length} turns · updated ${formatTime(task.updatedAtUtc)}`} actions={<StatusBadge state={task.status} />} /><ProblemBanner message={problem} clear={() => setProblem('')} />
    <section className="panel control-panel" aria-label="Task controls"><label>Execution mode<select value={result.data.executionMode ?? 'approval'} onChange={(event) => void action('execution-mode', { mode: event.target.value })} disabled={!!busy}><option value="approval">Ask for approval</option><option value="safe">Allow safe tools</option><option value="unrestricted">Unrestricted</option></select></label><div className="button-row"><button className="button secondary" disabled={!!busy} onClick={() => void action('approval', { decision: 'approve' })}><Check />Approve once</button><button className="button secondary" disabled={!!busy} onClick={() => void action('approval', { decision: 'reject' })}><Ban />Reject</button><button className="button danger" disabled={!!busy || task.status !== 'running'} onClick={() => void action('stop')}><CircleStop />Stop conversation</button></div></section>
    <section className="panel"><header className="panel-title"><div><span className="eyebrow">GENERATIONS & TURNS</span><h2>Activity timeline</h2></div></header>{turns.length ? <div className="turn-list">{turns.map((turn) => <article className="turn-card" key={turn.id}><header><div><strong>Generation {turn.generation ?? 1} · Turn {turn.id}</strong><span>{turn.actor ?? 'agent'} · {formatTime(turn.startedAtUtc)}</span></div><StatusBadge state={turn.status ?? 'unknown'} /></header><EventList events={turn.events ?? []} /></article>)}</div> : <EventList events={events} />}</section>
  </div>;
}
export function EventList({ events }: { events: TimelineEvent[] }) { return events.length ? <div className="event-stack">{events.map((event) => <Disclosure key={event.id} title={`${event.type} · ${formatTime(event.occurredAt)}`}><pre>{typeof event.payload === 'string' ? event.payload : JSON.stringify(event.payload ?? {}, null, 2)}</pre></Disclosure>)}</div> : <div className="inline-empty">No timeline events.</div>; }
