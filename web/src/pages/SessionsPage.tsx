import { CircleStop, Radio, Search, Send, TerminalSquare } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api';
import { Empty, ErrorState, Loading, PageHeading, ProblemBanner, StatusBadge, formatTime } from '../components';
import { useLoad } from '../useLoad';

const WINDOW_SIZE = 300;
export function SessionsPage() {
  const result = useLoad(api.sessions, []); const [query, setQuery] = useState('');
  const sessions = useMemo(() => [...(result.data ?? [])].sort((a, b) => Date.parse(b.updatedAtUtc ?? b.createdAtUtc ?? '') - Date.parse(a.updatedAtUtc ?? a.createdAtUtc ?? '')).filter((session) => `${session.id} ${session.shell ?? ''} ${session.workingDirectory ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, result.data]);
  return <div><PageHeading eyebrow="LOCAL PROCESSES" title="Sessions" body="Shell processes, working directories, and replayable terminal streams." /><div className="filter-bar"><label className="search-field"><Search /><span className="sr-only">Search sessions</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search ID, shell, or folder" /></label></div>{result.loading ? <Loading label="Loading sessions" /> : result.error ? <ErrorState message={result.error} retry={() => void result.reload()} /> : !sessions.length ? <Empty title="No terminal sessions" body="Sessions started through local MCP appear here." /> : <div className="record-list">{sessions.map((session) => <Link className="record-row" to={`/sessions/${encodeURIComponent(session.id)}`} key={session.id}><span className="record-icon"><TerminalSquare /></span><div className="record-main"><strong>{session.shell ?? 'Terminal session'}</strong><code>{session.id}</code><small>{session.workingDirectory ?? 'Working directory not reported'}</small></div><div className="record-meta"><StatusBadge state={session.status} /><time>{formatTime(session.updatedAtUtc ?? session.createdAtUtc)}</time></div></Link>)}</div>}</div>;
}
export function SessionDetailPage() {
  const { sessionId = '' } = useParams(); const result = useLoad(() => api.session(sessionId), [sessionId]); const [problem, setProblem] = useState(''); const [signal, setSignal] = useState('SIGINT'); const [visible, setVisible] = useState(WINDOW_SIZE);
  const action = async (name: string, body?: unknown) => { setProblem(''); try { result.setData(await api.sessionAction(sessionId, name, body)); } catch (error) { setProblem(error instanceof Error ? error.message : 'Action failed'); } };
  if (result.loading) return <Loading label="Loading terminal stream" />; if (result.error || !result.data) return <ErrorState message={result.error} retry={() => void result.reload()} />;
  const { session, events } = result.data; const shown = events.slice(Math.max(0, events.length - visible));
  return <div><PageHeading eyebrow={`SESSION · ${session.id}`} title={session.shell ?? 'Terminal session'} body={`${session.workingDirectory ?? 'Unknown folder'} · PID ${session.processId ?? '—'} · ${formatTime(session.createdAtUtc)}`} actions={<StatusBadge state={session.status} />} /><ProblemBanner message={problem} clear={() => setProblem('')} />
    <section className="panel session-controls"><label>Process signal<select value={signal} onChange={(event) => setSignal(event.target.value)}><option>SIGINT</option><option>SIGTERM</option><option>SIGKILL</option></select></label><button className="button secondary" onClick={() => void action('signal', { signal })}><Send />Send signal</button><button className="button danger" onClick={() => void action('close')}><CircleStop />Close session</button></section>
    <section className="terminal-panel" aria-label="Terminal event stream"><header><div><Radio /><strong>Terminal stream</strong><span>{shown.length} of {events.length} events rendered</span></div>{visible < events.length && <button className="button secondary compact" onClick={() => setVisible((value) => Math.min(events.length, value + WINDOW_SIZE))}>Load {Math.min(WINDOW_SIZE, events.length - visible)} older</button>}</header><div className="terminal-stream">{shown.length ? shown.map((event) => <div className="terminal-line" key={event.id}><time>{formatTime(event.occurredAt)}</time><strong>{event.type}</strong><pre>{typeof event.payload === 'string' ? event.payload : JSON.stringify(event.payload ?? {})}</pre></div>) : <div className="inline-empty">No output events.</div>}</div></section>
  </div>;
}
