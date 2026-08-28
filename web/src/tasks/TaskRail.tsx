import { Bot, Braces, ChevronUp, LayoutDashboard, LoaderCircle, Plus, Search, Settings, Sparkles, TerminalSquare, UserRound, Wrench, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, NavLink, useLocation } from 'react-router-dom';

import { api } from '../api';
import { Empty, ErrorState, Loading } from '../components';
import { useRealtime } from '../realtime';
import type { Task, TimelineEvent } from '../types';
import { upsertTaskEvent } from './taskTimeline';

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
  const location = useLocation();
  const taskId = activeTaskId(location.pathname);
  const [loadedTasks, setLoadedTasks] = useState<Task[]>([]);
  const [nextCursor, setNextCursor] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
  const [readFinalCounts, setReadFinalCounts] = useState<Record<string, number>>(readStoredFinalCounts);
  const visibleTaskIds = useRef(new Set<string>());
  const loadingMoreRef = useRef(false);
  const hadStoredReadCounts = useRef(typeof localStorage !== 'undefined' && localStorage.getItem(READ_FINAL_COUNTS_KEY) !== null);

  const applyFirstPage = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const page = await api.tasks(undefined, PAGE_SIZE);
      setLoadedTasks(pageItems(page).filter((task) => !task.isSubagent));
      setNextCursor(pageCursor(page));
    } catch (value) {
      setError(value instanceof Error ? value.message : 'Không thể tải đoạn trò chuyện.');
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshHead = useCallback(async () => {
    try {
      const page = await api.tasks(undefined, PAGE_SIZE);
      setLoadedTasks((current) => mergeTasks(pageItems(page).filter((task) => !task.isSubagent), current));
      if (!loadedTasks.length) setNextCursor(pageCursor(page));
    } catch { /* realtime refresh is best effort */ }
  }, [loadedTasks.length]);

  const loadMore = useCallback(async () => {
    if (!nextCursor || loadingMoreRef.current) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    setError('');
    try {
      const page = await api.tasks(nextCursor, PAGE_SIZE);
      setLoadedTasks((current) => mergeTasks(current, pageItems(page).filter((task) => !task.isSubagent)));
      setNextCursor(pageCursor(page));
    } catch (value) {
      setError(value instanceof Error ? value.message : 'Không thể tải thêm đoạn trò chuyện.');
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
    }
  }, [nextCursor]);

  useEffect(() => { void applyFirstPage(); }, [applyFirstPage]);
  useEffect(() => { visibleTaskIds.current = new Set(loadedTasks.map((task) => task.id)); }, [loadedTasks]);
  useEffect(() => {
    if (loading || hadStoredReadCounts.current) return;
    hadStoredReadCounts.current = true;
    setReadFinalCounts(Object.fromEntries(loadedTasks.map((task) => [task.id, task.finalResponseCount ?? 0])));
  }, [loadedTasks, loading]);

  const handleRealtime = useCallback((event: TimelineEvent) => {
    if (event.type === 'system.connected') { void refreshHead(); return; }
    if (!event.taskId) return;
    if (visibleTaskIds.current.has(event.taskId)) {
      setLoadedTasks((current) => upsertTaskEvent(current, event) ?? current);
    } else {
      void refreshHead();
    }
  }, [refreshHead]);
  useRealtime(handleRealtime);

  useEffect(() => {
    try { localStorage.setItem(READ_FINAL_COUNTS_KEY, JSON.stringify(readFinalCounts)); } catch { /* storage can be unavailable */ }
  }, [readFinalCounts]);

  useEffect(() => {
    if (!taskId) return;
    const task = loadedTasks.find((item) => item.id === taskId);
    if (!task) return;
    const count = task.finalResponseCount ?? 0;
    setReadFinalCounts((current) => (current[taskId] ?? 0) >= count ? current : { ...current, [taskId]: count });
  }, [taskId, loadedTasks]);

  useEffect(() => { setMenuOpen(false); onClose(); }, [location.pathname, onClose]);

  const tasks = useMemo(() => [...loadedTasks]
    .sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc))
    .filter((task) => `${conversationName(task)} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, loadedTasks]);

  return <aside className={`task-rail ${open ? 'open' : ''}`} aria-label="Đoạn trò chuyện">
    <header className="task-rail-header">
      <div className="task-rail-brand"><span className="brand-mark"><Braces /></span><strong>ChatCMD</strong><button className="icon-button mobile-only" aria-label="Close navigation" onClick={onClose}><X /></button></div>
      <Link className="task-rail-new-message" to="/tasks/new"><span className="task-rail-new-icon"><Plus /></span><span>Tin nhắn mới</span></Link>
      <label className="tasks-conversation-search"><Search /><span className="sr-only">Tìm đoạn trò chuyện</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Tìm kiếm" /></label>
    </header>

    <div className="task-rail-body">
      <div className="task-rail-section-label"><span>Đoạn trò chuyện</span><small>{tasks.length}</small></div>
      <div className="task-rail-list" onScroll={(event) => {
        const target = event.currentTarget;
        if (target.scrollHeight - target.scrollTop - target.clientHeight < 180) void loadMore();
      }}>
        {loading ? <Loading label="Loading tasks" /> : error && !tasks.length ? <ErrorState message={error} retry={() => void applyFirstPage()} /> : !tasks.length ? <Empty title="Chưa có đoạn trò chuyện" body="Đoạn trò chuyện từ Agent sẽ xuất hiện tại đây." /> : <>
          {tasks.map((task) => <TaskRailRow task={task} selected={task.id === taskId} unread={Math.max(0, (task.finalResponseCount ?? 0) - (readFinalCounts[task.id] ?? 0))} key={task.id} />)}
          {loadingMore && <div className="task-rail-load-more" role="status"><LoaderCircle className="spin" /><span>Đang tải thêm…</span></div>}
          {error && <button className="task-rail-load-retry" type="button" onClick={() => void loadMore()}>Tải lại</button>}
        </>}
      </div>
    </div>

    <footer className="task-rail-footer">
      {menuOpen && <nav className="task-rail-account-menu" aria-label="Application navigation">{menuItems.map(({ to, end, label, icon: Icon }) => <NavLink to={to} end={end} key={to}><Icon /><span>{label}</span></NavLink>)}</nav>}
      <button className="task-rail-account" type="button" aria-expanded={menuOpen} onClick={() => setMenuOpen((value) => !value)}>
        <span className="task-rail-avatar"><UserRound /></span>
        <span className="task-rail-account-copy"><strong>Tùng</strong><small>Pro plan</small></span>
        <ChevronUp className={menuOpen ? 'open' : ''} />
      </button>
    </footer>
  </aside>;
}

function TaskRailRow({ task, selected, unread }: { task: Task; selected: boolean; unread: number }) {
  const running = task.status === 'running';
  return <Link className={`tasks-conversation-row ${selected ? 'selected' : ''} ${unread > 0 ? 'unread' : ''}`} aria-current={selected ? 'page' : undefined} to={`/tasks/${encodeURIComponent(task.id)}`}>
    <span className="tasks-conversation-copy">
      <span className="tasks-conversation-title-row"><strong>{conversationName(task)}</strong>{unread > 0 && <span className="task-unread-badge" aria-label={`${unread} phản hồi mới chưa đọc`}>{unread > 99 ? '99+' : unread}</span>}</span>
      <small>{task.outputPreview || `${task.turnCount ?? 0} lượt Agent`}</small>
      <span className="tasks-conversation-status-line"><span className={`task-rail-state ${task.status}`}>{running ? <LoaderCircle className="spin" /> : <i />}</span><span>{taskStatusLabel(task.status)}</span></span>
    </span>
  </Link>;
}

function pageItems(page: { items?: Task[] } | Task[]) { return Array.isArray(page) ? page : page.items ?? []; }
function pageCursor(page: { nextCursor?: string } | Task[]) { return Array.isArray(page) ? undefined : page.nextCursor; }

function mergeTasks(first: Task[], second: Task[]) {
  const merged = new Map<string, Task>();
  for (const task of [...first, ...second]) if (!merged.has(task.id)) merged.set(task.id, task);
  return [...merged.values()];
}

function taskStatusLabel(status: string) {
  if (status === 'running') return 'Đang xử lý';
  if (status === 'completed') return 'Hoàn tất';
  if (status === 'failed') return 'Có lỗi';
  if (status === 'stopped') return 'Đã dừng';
  return status;
}

function activeTaskId(pathname: string) {
  if (!pathname.startsWith('/tasks/')) return undefined;
  const value = pathname.slice('/tasks/'.length).split('/')[0];
  if (!value || value === 'new') return undefined;
  try { return decodeURIComponent(value); } catch { return value; }
}

function readStoredFinalCounts(): Record<string, number> {
  try {
    const value = JSON.parse(localStorage.getItem(READ_FINAL_COUNTS_KEY) ?? '{}') as Record<string, unknown>;
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] >= 0));
  } catch { return {}; }
}

function conversationName(task: Task) { return task.agentName?.trim() || task.title?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = ['Mây', 'Sao', 'Gió', 'Nắng', 'Trăng', 'Biển', 'Rừng', 'Sương']; const second = ['Xanh', 'Nhẹ', 'Sớm', 'Đêm', 'Mới', 'Xa', 'Ấm', 'Sáng']; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
