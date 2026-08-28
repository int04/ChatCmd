import { Activity, Bot, Database, HardDrive, Radio, Server, ShieldAlert, TerminalSquare } from 'lucide-react';
import { useCallback, useState } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api';
import { ErrorState, Loading, PageHeading, StatusBadge, formatTime, healthLabel } from '../components';
import { formatAppNumber, tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { TimelineEvent } from '../types';
import { useLoad } from '../useLoad';

export function DashboardPage() {
  const overview = useLoad(api.overview, []); const [liveEvents, setLiveEvents] = useState<TimelineEvent[]>([]);
  const mergeEvent = useCallback((event: TimelineEvent) => setLiveEvents((current) => current.some((item) => item.id === event.id) ? current : [event, ...current].slice(0, 50)), []);
  const realtime = useRealtime(mergeEvent);
  if (overview.loading) return <Loading label={tr('Loading local runtime')} />;
  if (overview.error || !overview.data) return <><PageHeading eyebrow={tr('LOCAL CONTROL')} title={tr('Runtime overview')} body={tr('Health and activity from this machine.')} /><ErrorState message={overview.error} retry={() => void overview.reload()} /></>;
  const data = overview.data; const events = [...liveEvents, ...(data.recentEvents ?? [])].filter((value, index, all) => all.findIndex((item) => item.id === value.id) === index).slice(0, 12);
  const degraded = !['ready', 'running'].includes(data.app.state) || data.mcp.state !== 'listening' || !['ready', 'running'].includes(data.database.state);
  return <div><PageHeading eyebrow={tr('LOCAL CONTROL')} title={tr('Runtime overview')} body={tr('AI, MCP, tasks, and terminal activity on this machine.')} actions={<StatusBadge state={degraded ? 'degraded' : 'ready'} label={degraded ? tr('Attention needed') : tr('Runtime ready')} />} />
    {degraded && <div className="degraded-banner" role="status"><ShieldAlert /><div><strong>{tr('Runtime is degraded')}</strong><span>{tr('One or more local services did not report a healthy state. Review cards below.')}</span></div></div>}
    <section className="card-grid overview-grid" aria-label={tr('Runtime health')}>
      <Metric icon={<Server />} label={tr('Local runtime')} value={healthLabel(data.app.state)} detail={`v${data.app.version} · ${formatTime(data.app.startedAtUtc)}`} state={data.app.state} />
      <Metric icon={<HardDrive />} label={tr('This device')} value={data.device.name} detail={`${data.device.platform} · ${data.device.architecture}`} state={data.app.state} />
      <Metric icon={<Radio />} label={tr('MCP listener')} value={healthLabel(data.mcp.state)} detail={data.mcp.endpoint ?? data.mcp.lastError ?? tr('Endpoint not reported')} state={data.mcp.state} />
      <Metric icon={<Database />} label="SQLite" value={healthLabel(data.database.state)} detail={`${data.database.path}${data.database.schemaVersion ? ` · schema ${data.database.schemaVersion}` : ''}`} state={data.database.state} />
      <Metric icon={<Activity />} label={tr('Tasks')} value={tr('{count} active', { count: formatAppNumber(data.tasks.running) })} detail={tr('{completed} completed · {failed} failed', { completed: formatAppNumber(data.tasks.completed), failed: formatAppNumber(data.tasks.failed) })} state={data.tasks.failed ? 'degraded' : 'ready'} />
      <Metric icon={<TerminalSquare />} label={tr('Sessions')} value={tr('{count} active', { count: formatAppNumber(data.terminal.activeSessions) })} detail={tr('{total} total · {shell}', { total: formatAppNumber(data.terminal.totalSessions), shell: data.terminal.defaultShell })} state={data.terminal.failedSessions ? 'degraded' : 'ready'} />
      <Metric icon={<Bot />} label={tr('MCP clients')} value={formatAppNumber(data.mcp.connectedClients)} detail={tr('Connected to local listener')} state={data.mcp.state} />
      <Metric icon={<ShieldAlert />} label={tr('Approvals')} value={formatAppNumber(data.tasks.approvals)} detail={tr('Waiting for local decision')} state={data.tasks.approvals ? 'degraded' : 'ready'} />
    </section>
    <section className="panel timeline-panel"><header className="panel-title"><div><span className="eyebrow">{tr('RECENT ACTIVITY')}</span><h2>{tr('Timeline')}</h2></div><StatusBadge state={realtime} /></header>{events.length ? <div className="timeline-list">{events.map((event) => <div className="timeline-row" key={event.id}><i aria-hidden="true" /><div><strong>{event.type}</strong><span>{event.taskId ? <>{tr('Task')} <Link to={`/tasks/${encodeURIComponent(event.taskId)}`}>{event.taskId}</Link></> : event.sessionId ? <>{tr('Session')} <Link to={`/sessions/${encodeURIComponent(event.sessionId)}`}>{event.sessionId}</Link></> : tr('Local runtime')}</span></div><time>{formatTime(event.occurredAt)}</time></div>)}</div> : <div className="inline-empty">{tr('No recent activity reported.')}</div>}</section>
  </div>;
}
function Metric({ icon, label, value, detail, state }: { icon: React.ReactNode; label: string; value: string; detail: string; state: string }) { return <article className="metric-card"><span className="metric-icon">{icon}</span><div><span>{label}</span><strong>{value}</strong><small title={detail}>{detail}</small></div><StatusBadge state={state} /></article>; }
