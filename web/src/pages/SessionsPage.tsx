import { Activity, CircleStop, Cpu, HardDrive, Radio, Search, Send, TerminalSquare } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api';
import { Empty, ErrorState, Loading, PageHeading, ProblemBanner, StatusBadge, formatTime } from '../components';
import { tr } from '../i18n';
import type { Session } from '../types';
import { useLoad } from '../useLoad';

const WINDOW_SIZE = 300;
const formatBytes = (value?: number) => value == null ? '—' : value < 1024 ** 2 ? `${Math.round(value / 1024)} KB` : value < 1024 ** 3 ? `${(value / 1024 ** 2).toFixed(1)} MB` : `${(value / 1024 ** 3).toFixed(2)} GB`;

export function SessionsPage() {
  const history = useLoad(api.sessions, []);
  const live = useLoad(api.liveTerminals, []);
  const [tab, setTab] = useState<'terminal' | 'sessions'>('terminal');
  const [query, setQuery] = useState('');
  const refreshLive = live.refresh;
  useEffect(() => { const timer = window.setInterval(() => void refreshLive(), 2000); return () => window.clearInterval(timer); }, [refreshLive]);
  const sessions = useMemo(() => {
    const source = tab === 'terminal' ? live.data ?? [] : history.data ?? [];
    return [...source].sort((a, b) => Date.parse(b.updatedAtUtc ?? b.createdAtUtc ?? '') - Date.parse(a.updatedAtUtc ?? a.createdAtUtc ?? '')).filter((session) => `${session.id} ${session.shell ?? ''} ${session.workingDirectory ?? ''} ${session.processId ?? ''}`.toLowerCase().includes(query.toLowerCase()));
  }, [history.data, live.data, query, tab]);
  const result = tab === 'terminal' ? live : history;
  return <div>
    <PageHeading eyebrow={tr('LOCAL ACTIVITY')} title={tr('Sessions')} body={tr('Manage logical sessions and currently running terminals.')} />
    <div className="sessions-tabs" role="tablist">
      <button className={`sessions-tab ${tab === 'terminal' ? 'active' : ''}`} onClick={() => setTab('terminal')}><TerminalSquare />{tr('Terminal')}<span>{live.data?.length ?? 0}</span></button>
      <button className={`sessions-tab ${tab === 'sessions' ? 'active' : ''}`} onClick={() => setTab('sessions')}><Radio />{tr('Sessions')}<span>{history.data?.length ?? 0}</span></button>
    </div>
    <div className="filter-bar"><label className="search-field"><Search /><span className="sr-only">{tr('Search sessions')}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={tab === 'terminal' ? tr('Search terminal, PID, or folder') : tr('Search ID, shell, or folder')} /></label></div>
    {result.loading && !result.data ? <Loading label={tr('Loading sessions')} /> : result.error && !result.data ? <ErrorState message={result.error} retry={() => void result.reload()} /> : !sessions.length ? <Empty title={tab === 'terminal' ? tr('No running terminals') : tr('No sessions')} body={tab === 'terminal' ? tr('Active CMD, PowerShell, and shell sessions will appear here.') : tr('MCP and terminal sessions appear here.')} /> : tab === 'terminal' ? <TerminalList sessions={sessions} /> : <HistoryList sessions={sessions} />}
  </div>;
}

function TerminalList({ sessions }: { sessions: Session[] }) {
  return <div className="terminal-live-grid">{sessions.map((session) => <Link className="terminal-live-card" to={`/sessions/terminal/${encodeURIComponent(session.id)}`} key={session.id}>
    <header><span className="terminal-live-icon"><TerminalSquare /></span><div><strong>{session.shell ?? tr('Terminal')}</strong><code>{session.id}</code></div><span className={`terminal-usage ${session.busy ? 'busy' : 'idle'}`}>{session.busy ? tr('In use') : tr('Idle')}</span><StatusBadge state={session.status} /></header>
    <p>{session.workingDirectory ?? tr('Working directory not reported')}</p>
    <div className="terminal-live-stats"><span><Activity />PID <b>{session.processId ?? '—'}</b></span><span><Cpu />CPU <b>{session.cpuPercent == null ? '—' : `${session.cpuPercent.toFixed(1)}%`}</b></span><span><HardDrive />RAM <b>{formatBytes(session.memoryBytes)}</b></span></div>
    <footer><span>{session.taskId ? `${tr('Task')} ${session.taskId}` : tr('Not linked to a task')}</span><span>{formatTime(session.createdAtUtc)}</span></footer>
  </Link>)}</div>;
}

function HistoryList({ sessions }: { sessions: Session[] }) {
  return <div className="record-list">{sessions.map((session) => <Link className="record-row" to={`/sessions/${encodeURIComponent(session.id)}`} key={`${session.kind}:${session.id}`}><span className="record-icon">{session.kind === 'mcp' ? <Radio /> : <TerminalSquare />}</span><div className="record-main"><strong>{session.kind === 'mcp' ? tr('MCP logical session') : session.shell ?? tr('Terminal session')}</strong><code>{session.id}</code><small>{session.kind === 'mcp' ? `${tr('Task')} ${session.taskId ?? tr('not linked')}` : session.workingDirectory ?? tr('Working directory not reported')}</small></div><div className="record-meta"><StatusBadge state={session.status} /><time>{formatTime(session.updatedAtUtc ?? session.createdAtUtc)}</time></div></Link>)}</div>;
}

export function SessionDetailPage() {
  const { sessionId = '' } = useParams(); const result = useLoad(() => api.session(sessionId), [sessionId]); const [problem, setProblem] = useState(''); const [signal, setSignal] = useState('SIGINT'); const [visible, setVisible] = useState(WINDOW_SIZE);
  const action = async (name: string, body?: unknown) => { setProblem(''); try { result.setData(await api.sessionAction(sessionId, name, body)); } catch (error) { setProblem(error instanceof Error ? error.message : tr('Action failed')); } };
  if (result.loading) return <Loading label={tr('Loading session')} />; if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;
  const { session, events } = result.data; const shown = events.slice(Math.max(0, events.length - visible));
  const isMcp = session.kind === 'mcp';
  return <div><PageHeading eyebrow={`${isMcp ? tr('MCP LOGICAL SESSION') : tr('TERMINAL SESSION')} · ${session.id}`} title={isMcp ? tr('MCP logical session') : session.shell ?? tr('Terminal session')} body={isMcp ? `${tr('Task')} ${session.taskId ?? tr('not linked')} · ${formatTime(session.createdAtUtc)}` : `${session.workingDirectory ?? tr('Unknown folder')} · PID ${session.processId ?? '—'} · ${formatTime(session.createdAtUtc)}`} actions={<StatusBadge state={session.status} />} /><ProblemBanner message={problem} clear={() => setProblem('')} />
    {!isMcp && <section className="panel session-controls"><label>{tr('Process signal')}<select value={signal} onChange={(event) => setSignal(event.target.value)}><option>SIGINT</option><option>SIGTERM</option><option>SIGKILL</option></select></label><button className="button secondary" onClick={() => void action('signal', { signal })}><Send />{tr('Send signal')}</button><button className="button danger" onClick={() => void action('close')}><CircleStop />{tr('Close session')}</button></section>}
    <section className="terminal-panel" aria-label={isMcp ? tr('MCP timeline') : tr('Terminal event stream')}><header><div><Radio /><strong>{isMcp ? tr('MCP timeline') : tr('Terminal stream')}</strong><span>{tr('{shown} of {total} events rendered', { shown: shown.length, total: events.length })}</span></div>{visible < events.length && <button className="button secondary compact" onClick={() => setVisible((value) => Math.min(events.length, value + WINDOW_SIZE))}>{tr('Load {count} older', { count: Math.min(WINDOW_SIZE, events.length - visible) })}</button>}</header><div className="terminal-stream">{shown.length ? shown.map((event) => <div className="terminal-line" key={event.id}><time>{formatTime(event.occurredAt)}</time><strong>{event.type}</strong><pre>{typeof event.payload === 'string' ? event.payload : JSON.stringify(event.payload ?? {})}</pre></div>) : <div className="inline-empty">{tr('No events.')}</div>}</div></section>
  </div>;
}
