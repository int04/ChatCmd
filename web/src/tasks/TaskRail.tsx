import { AlertTriangle, Bot, Braces, ChevronUp, LayoutDashboard, LoaderCircle, Plus, Search, Settings, Sparkles, TerminalSquare, Trash2, UserRound, Wrench, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEventHandler } from 'react';
import { Link, NavLink, useLocation, useNavigate } from 'react-router-dom';

import { api } from '../api';
import { Empty, ErrorState, Loading, Modal } from '../components';
import { formatAppNumber, tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { Task, TimelineEvent } from '../types';
import { upsertTaskEvent } from './taskTimeline';
import { useResizableWidth } from './useResizableWidth';

const READ_FINAL_COUNTS_KEY = 'chatcmd.tasks.readFinalCounts.v1';
const PAGE_SIZE = 10;
const menuItems = [
  { to: '/', end: true, label: 'Overview', icon: LayoutDashboard },
  { to: '/tasks', label: 'Task', icon: Sparkles },
  { to: '/sessions', label: 'Session', icon: TerminalSquare },
  { to: '/agents', label: 'Agents', icon: Bot },
  { to: '/skills', label: 'Skills', icon: Wrench },
  { to: '/settings', label: 'Setting', icon: Settings },
];

export function TaskRail({ open, onClose }: { open: boolean; onClose: () => void }) {
  const location = useLocation(); const navigate = useNavigate(); const taskId = activeTaskId(location.pathname);
  const [loadedTasks, setLoadedTasks] = useState<Task[]>([]); const [nextCursor, setNextCursor] = useState<string>(); const [loading, setLoading] = useState(true); const [loadingMore, setLoadingMore] = useState(false); const [error, setError] = useState(''); const [query, setQuery] = useState(''); const [menuOpen, setMenuOpen] = useState(false); const [contextMenu, setContextMenu] = useState<{ task: Task; x: number; y: number }>(); const [deleteTarget, setDeleteTarget] = useState<Task>(); const [deleting, setDeleting] = useState(false); const [deleteError, setDeleteError] = useState('');
  const [readFinalCounts, setReadFinalCounts] = useState<Record<string, number>>(readStoredFinalCounts);
  const visibleTaskIds = useRef(new Set<string>()); const loadingMoreRef = useRef(false); const hadStoredReadCounts = useRef(typeof localStorage !== 'undefined' && localStorage.getItem(READ_FINAL_COUNTS_KEY) !== null);
  const railResize = useResizableWidth({ storageKey: 'chatcmd.layout.taskRailWidth.v1', cssVariable: '--task-rail-width', defaultWidth: typeof window !== 'undefined' && window.innerWidth <= 1180 ? 270 : 284, minWidth: 240, maxWidth: 480 });

  const applyFirstPage = useCallback(async () => {
    setLoading(true); setError('');
    try { const page = await api.tasks(undefined, PAGE_SIZE); setLoadedTasks(pageItems(page).filter((task) => !task.isSubagent)); setNextCursor(pageCursor(page)); }
    catch (value) { setError(value instanceof Error ? value.message : tr('Could not load conversations.')); }
    finally { setLoading(false); }
  }, []);
  const refreshHead = useCallback(async () => { try { const page = await api.tasks(undefined, PAGE_SIZE); setLoadedTasks((current) => mergeTasks(pageItems(page).filter((task) => !task.isSubagent), current)); if (!loadedTasks.length) setNextCursor(pageCursor(page)); } catch { /* best effort */ } }, [loadedTasks.length]);
  const loadMore = useCallback(async () => {
    if (!nextCursor || loadingMoreRef.current) return; loadingMoreRef.current = true; setLoadingMore(true); setError('');
    try { const page = await api.tasks(nextCursor, PAGE_SIZE); setLoadedTasks((current) => mergeTasks(current, pageItems(page).filter((task) => !task.isSubagent))); setNextCursor(pageCursor(page)); }
    catch (value) { setError(value instanceof Error ? value.message : tr('Could not load more conversations.')); }
    finally { loadingMoreRef.current = false; setLoadingMore(false); }
  }, [nextCursor]);

  useEffect(() => { void applyFirstPage(); }, [applyFirstPage]);
  useEffect(() => { visibleTaskIds.current = new Set(loadedTasks.map((task) => task.id)); }, [loadedTasks]);
  useEffect(() => { if (loading || hadStoredReadCounts.current) return; hadStoredReadCounts.current = true; setReadFinalCounts(Object.fromEntries(loadedTasks.map((task) => [task.id, task.finalResponseCount ?? 0]))); }, [loadedTasks, loading]);
  const handleRealtime = useCallback((event: TimelineEvent) => { if (event.type === 'system.connected') { void refreshHead(); return; } if (!event.taskId) return; if (visibleTaskIds.current.has(event.taskId)) setLoadedTasks((current) => upsertTaskEvent(current, event) ?? current); else void refreshHead(); }, [refreshHead]);
  useRealtime(handleRealtime);
  useEffect(() => { try { localStorage.setItem(READ_FINAL_COUNTS_KEY, JSON.stringify(readFinalCounts)); } catch { /* unavailable */ } }, [readFinalCounts]);
  useEffect(() => { if (!taskId) return; const task = loadedTasks.find((item) => item.id === taskId); if (!task) return; const count = task.finalResponseCount ?? 0; setReadFinalCounts((current) => (current[taskId] ?? 0) >= count ? current : { ...current, [taskId]: count }); }, [taskId, loadedTasks]);
  useEffect(() => { setMenuOpen(false); setContextMenu(undefined); onClose(); }, [location.pathname, onClose]);
  useEffect(() => { if (!contextMenu) return; const close = () => setContextMenu(undefined); window.addEventListener('pointerdown', close); window.addEventListener('blur', close); return () => { window.removeEventListener('pointerdown', close); window.removeEventListener('blur', close); }; }, [contextMenu]);

  const deleteConversation = useCallback(async () => {
    if (!deleteTarget || !canDeleteTask(deleteTarget)) return; setDeleting(true); setDeleteError('');
    try { await api.deleteTask(deleteTarget.id); setLoadedTasks((current) => current.filter((task) => task.id !== deleteTarget.id)); setReadFinalCounts((current) => { const next = { ...current }; delete next[deleteTarget.id]; return next; }); if (taskId === deleteTarget.id) navigate('/tasks'); setDeleteTarget(undefined); }
    catch (value) { setDeleteError(value instanceof Error ? value.message : tr('Could not delete conversation.')); }
    finally { setDeleting(false); }
  }, [deleteTarget, navigate, taskId]);

  const tasks = useMemo(() => [...loadedTasks].sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc)).filter((task) => `${conversationName(task)} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, loadedTasks]);

  return <aside className={`task-rail ${open ? 'open' : ''}`} aria-label={tr('Conversations')}>
    <div className="panel-resize-handle task-rail-resize-handle" role="separator" aria-label={tr('Resize conversations')} aria-orientation="vertical" aria-valuemin={240} aria-valuemax={480} aria-valuenow={railResize.width} tabIndex={0} onPointerDown={railResize.onPointerDown} onKeyDown={railResize.onKeyDown} />
    <header className="task-rail-header">
      <div className="task-rail-brand"><span className="brand-mark"><Braces /></span><strong>ChatCMD</strong><button className="icon-button mobile-only" aria-label={tr('Close navigation')} onClick={onClose}><X /></button></div>
      <Link className="task-rail-new-message" to="/tasks/new"><span className="task-rail-new-icon"><Plus /></span><span>{tr('New message')}</span></Link>
      <label className="tasks-conversation-search"><Search /><span className="sr-only">{tr('Search conversations')}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={tr('Search')} /></label>
    </header>
    <div className="task-rail-body"><div className="task-rail-section-label"><span>{tr('Conversations')}</span></div><div className="task-rail-list" onScroll={(event) => { const target = event.currentTarget; if (target.scrollHeight - target.scrollTop - target.clientHeight < 180) void loadMore(); }}>
      {loading ? <Loading label={tr('Loading tasks')} /> : error && !tasks.length ? <ErrorState message={error} retry={() => void applyFirstPage()} /> : !tasks.length ? <Empty title={tr('No conversations yet')} body={tr('Agent conversations will appear here.')} /> : <>
        {tasks.map((task) => <TaskRailRow task={task} selected={task.id === taskId} unread={Math.max(0, (task.finalResponseCount ?? 0) - (readFinalCounts[task.id] ?? 0))} onContextMenu={(event) => { event.preventDefault(); setContextMenu({ task, x: Math.min(event.clientX, window.innerWidth - 236), y: Math.min(event.clientY, window.innerHeight - 108) }); }} key={task.id} />)}
        {loadingMore && <div className="task-rail-load-more" role="status"><LoaderCircle className="spin" /><span>{tr('Loading more…')}</span></div>}
        {error && <button className="task-rail-load-retry" type="button" onClick={() => void loadMore()}>{tr('Reload')}</button>}
      </>}
    </div></div>
    {contextMenu && <div className="task-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }} onPointerDown={(event) => event.stopPropagation()}><button type="button" role="menuitem" className="danger" disabled={!canDeleteTask(contextMenu.task)} onClick={() => { setDeleteError(''); setDeleteTarget(contextMenu.task); setContextMenu(undefined); }}><Trash2 /><span>{tr('Delete conversation')}</span></button>{!canDeleteTask(contextMenu.task) && <small>{tr('You can only delete a task after it has finished.')}</small>}</div>}
    {deleteTarget && <Modal title={tr('Delete conversation?')} description={conversationName(deleteTarget)} close={() => !deleting && setDeleteTarget(undefined)} dangerous><div className="task-delete-warning"><AlertTriangle /><div><strong>{tr('Warning')}</strong><p>{tr('Deleting removes this conversation and its linked data from the list. This conversation may not work again in the future.')}</p></div></div>{deleteError && <p className="task-delete-error" role="alert">{deleteError}</p>}<div className="modal-actions"><button className="button secondary" type="button" disabled={deleting} onClick={() => setDeleteTarget(undefined)}>{tr('Cancel')}</button><button className="button danger" type="button" disabled={deleting} onClick={() => void deleteConversation()}>{deleting ? tr('Deleting…') : tr('Delete conversation')}</button></div></Modal>}
    <footer className="task-rail-footer">{menuOpen && <nav className="task-rail-account-menu" aria-label={tr('Application navigation')}>{menuItems.map(({ to, end, label, icon: Icon }) => <NavLink to={to} end={end} key={to}><Icon /><span>{tr(label)}</span></NavLink>)}</nav>}<button className="task-rail-account" type="button" aria-expanded={menuOpen} onClick={() => setMenuOpen((value) => !value)}><span className="task-rail-avatar"><UserRound /></span><span className="task-rail-account-copy"><strong>Tùng</strong><small>{tr('Pro plan')}</small></span><ChevronUp className={menuOpen ? 'open' : ''} /></button></footer>
  </aside>;
}

function TaskRailRow({ task, selected, unread, onContextMenu }: { task: Task; selected: boolean; unread: number; onContextMenu: MouseEventHandler<HTMLAnchorElement> }) {
  const running = task.status === 'running';
  return <Link className={`tasks-conversation-row ${selected ? 'selected' : ''} ${unread > 0 ? 'unread' : ''}`} aria-current={selected ? 'page' : undefined} to={`/tasks/${encodeURIComponent(task.id)}`} onContextMenu={onContextMenu}><span className="tasks-conversation-copy"><span className="tasks-conversation-title-row"><strong>{conversationName(task)}</strong>{unread > 0 && <span className="task-unread-badge" aria-label={tr('{count} unread final responses', { count: unread })}>{unread > 99 ? '99+' : unread}</span>}</span><small>{task.outputPreview || tr('{count} Agent turns', { count: formatAppNumber(task.turnCount ?? 0) })}</small><span className="tasks-conversation-status-line"><span className={`task-rail-state ${task.status}`}>{running ? <LoaderCircle className="spin" /> : <i />}</span><span>{taskStatusLabel(task.status)}</span></span></span></Link>;
}
function pageItems(page: { items?: Task[] } | Task[]) { return Array.isArray(page) ? page : page.items ?? []; }
function pageCursor(page: { nextCursor?: string } | Task[]) { return Array.isArray(page) ? undefined : page.nextCursor; }
function mergeTasks(first: Task[], second: Task[]) { const merged = new Map<string, Task>(); for (const task of [...first, ...second]) if (!merged.has(task.id)) merged.set(task.id, task); return [...merged.values()]; }
function canDeleteTask(task: Task) { return ['completed', 'failed', 'stopped', 'interrupted'].includes(task.status); }
function taskStatusLabel(status: string) { if (status === 'running') return tr('Processing'); if (status === 'completed') return tr('Complete'); if (status === 'failed') return tr('Has errors'); if (status === 'stopped') return tr('Stopped'); return status; }
function activeTaskId(pathname: string) { if (!pathname.startsWith('/tasks/')) return undefined; const value = pathname.slice('/tasks/'.length).split('/')[0]; if (!value || value === 'new') return undefined; try { return decodeURIComponent(value); } catch { return value; } }
function readStoredFinalCounts(): Record<string, number> { try { const value = JSON.parse(localStorage.getItem(READ_FINAL_COUNTS_KEY) ?? '{}') as Record<string, unknown>; return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] >= 0)); } catch { return {}; } }
function conversationName(task: Task) { return task.agentName?.trim() || task.title?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = [tr('Cloud'), tr('Star'), tr('Wind'), tr('Sun'), tr('Moon'), tr('Sea'), tr('Forest'), tr('Mist')]; const second = [tr('Blue'), tr('Soft'), tr('Morning'), tr('Night'), tr('New'), tr('Far'), tr('Warm'), tr('Bright')]; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
