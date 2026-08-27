import { Bot, Braces, LayoutDashboard, Menu, Settings, Sparkles, TerminalSquare, Wrench, X } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { NavLink, Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { AgentsPage } from './pages/AgentsPage';
import { DashboardPage } from './pages/DashboardPage';
import { SessionsPage, SessionDetailPage } from './pages/SessionsPage';
import { SettingsPage } from './pages/SettingsPage';
import { SkillsPage } from './pages/SkillsPage';
import { TasksPage } from './pages/TasksPage';
import { RealtimeProvider, useRealtime } from './realtime';
import { soundNotifications } from './soundNotifications';
import type { TimelineEvent } from './types';

const legacyPaths = ['/login', '/register', '/plans', '/account', '/payment', '/payments', '/purchase', '/checkout'];
const nav = [
  { to: '/', end: true, label: 'Overview', icon: LayoutDashboard },
  { to: '/tasks', label: 'Tasks', icon: Sparkles },
  { to: '/sessions', label: 'Sessions', icon: TerminalSquare },
  { to: '/agents', label: 'Agents', icon: Bot },
  { to: '/skills', label: 'Skills', icon: Wrench },
  { to: '/settings', label: 'Settings', icon: Settings },
];
export default function App() { return <RealtimeProvider><SoundNotificationsBridge /><Routes><Route element={<Shell />}><Route index element={<DashboardPage />} /><Route path="tasks/:taskId?" element={<TasksPage />} /><Route path="sessions" element={<SessionsPage />} /><Route path="sessions/:sessionId" element={<SessionDetailPage />} /><Route path="agents" element={<AgentsPage />} /><Route path="skills" element={<SkillsPage />} /><Route path="settings" element={<SettingsPage />} />{legacyPaths.map((path) => <Route key={path} path={path} element={<Navigate replace to="/" />} />)}<Route path="*" element={<NotFound />} /></Route></Routes></RealtimeProvider>; }
function SoundNotificationsBridge() {
  const onEvent = useCallback((event: TimelineEvent) => {
    const payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {};
    if (event.type === 'status' && payload.status === 'completed') soundNotifications.playFinishedTask();
    if (event.type === 'message' && payload.role === 'user' && typeof payload.title === 'string' && payload.title.trim()) soundNotifications.playNewAgent();
  }, []);
  useRealtime(onEvent);
  return null;
}
function Shell() { const [open, setOpen] = useState(false); const location = useLocation(); useEffect(() => { setOpen(false); requestAnimationFrame(() => document.getElementById('main-content')?.focus({ preventScroll: true })); }, [location.pathname]); useEffect(() => { try { const saved = JSON.parse(localStorage.getItem('chatcmd.preferences') ?? '{}') as { theme?: string }; if (saved.theme) document.documentElement.dataset.theme = saved.theme; } catch { /* invalid local preference */ } }, []);
  return <div className="shell"><a className="skip-link" href="#main-content">Skip to content</a><aside className={`sidebar ${open ? 'open' : ''}`}><header className="brand"><span className="brand-mark"><Braces /></span><span><strong>ChatCMD</strong><small>Local MCP Console</small></span><button className="icon-button mobile-only" aria-label="Close navigation" onClick={() => setOpen(false)}><X /></button></header><nav aria-label="Primary navigation">{nav.map(({ to, end, label, icon: Icon }) => <NavLink to={to} end={end} key={to}><Icon /><span>{label}</span></NavLink>)}</nav><footer><span className="local-device"><i />This machine</span><small>Single-user local runtime</small></footer></aside>{open && <button className="scrim" aria-label="Close navigation" onClick={() => setOpen(false)} />}<div className="content-shell"><header className="mobile-topbar"><button className="icon-button" aria-label="Open navigation" onClick={() => setOpen(true)}><Menu /></button><strong>ChatCMD</strong><span>Local</span></header><main id="main-content" className={location.pathname.startsWith('/tasks') ? 'tasks-main' : undefined} tabIndex={-1}><Outlet /></main></div><nav className="mobile-nav" aria-label="Mobile navigation">{nav.slice(0, 4).map(({ to, end, label, icon: Icon }) => <NavLink to={to} end={end} key={to}><Icon /><span>{label}</span></NavLink>)}<button aria-label="Open more navigation" onClick={() => setOpen(true)}><Menu /><span>More</span></button></nav></div>;
}
function NotFound() { return <div className="state-panel"><Sparkles /><strong>Page not found</strong><span>This local route does not exist.</span><NavLink className="button primary" to="/">Open overview</NavLink></div>; }
