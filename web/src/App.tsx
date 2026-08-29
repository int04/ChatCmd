import { Menu, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { NavLink, Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { useAppLanguage, tr } from './i18n';
import { AgentsPage } from './pages/AgentsPage';
import { DashboardPage } from './pages/DashboardPage';
import { SessionsPage, SessionDetailPage } from './pages/SessionsPage';
import { SettingsPage } from './pages/SettingsPage';
import { SkillsPage } from './pages/SkillsPage';
import { TasksPage } from './pages/TasksPage';
import { RealtimeProvider, useRealtime } from './realtime';
import { soundNotifications } from './soundNotifications';
import { GlobalConversationApprovalQueue } from './tasks/GlobalConversationApprovalQueue';
import { TaskRail } from './tasks/TaskRail';
import { useTaskDocumentTitle } from './tasks/taskDocumentTitle';
import type { TimelineEvent } from './types';

const legacyPaths = ['/login', '/register', '/plans', '/account', '/payment', '/payments', '/purchase', '/checkout'];
export default function App() {
  useAppLanguage();
  return <RealtimeProvider><GlobalDocumentTitleBridge /><SoundNotificationsBridge /><GlobalConversationApprovalQueue /><Routes><Route element={<Shell />}><Route index element={<DashboardPage />} /><Route path="tasks/:taskId?" element={<TasksPage />} /><Route path="sessions" element={<SessionsPage />} /><Route path="sessions/:sessionId" element={<SessionDetailPage />} /><Route path="agents" element={<AgentsPage />} /><Route path="skills" element={<SkillsPage />} /><Route path="settings" element={<SettingsPage />} />{legacyPaths.map((path) => <Route key={path} path={path} element={<Navigate replace to="/" />} />)}<Route path="*" element={<NotFound />} /></Route></Routes></RealtimeProvider>;
}
function GlobalDocumentTitleBridge() {
  const updateDocumentTitle = useTaskDocumentTitle(undefined);
  const handleEvent = useCallback((event: TimelineEvent) => {
    if (document.documentElement.dataset.approvalRequired === 'true') return;
    updateDocumentTitle(event);
  }, [updateDocumentTitle]);
  useRealtime(handleEvent);
  return null;
}
function SoundNotificationsBridge() {
  const onEvent = useCallback((event: TimelineEvent) => {
    const payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {};
    if (isNewConversationEvent(event, payload)) soundNotifications.playNewAgent();
    if (isFinalResponseEvent(event, payload)) soundNotifications.playFinishedTask();
  }, []);
  useRealtime(onEvent);
  return null;
}
export function isNewConversationEvent(event: TimelineEvent, payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {}) {
  return event.type === 'message' && payload.role === 'user' && typeof payload.title === 'string' && payload.title.trim().length > 0;
}
export function isFinalResponseEvent(event: TimelineEvent, payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {}) {
  if (event.type !== 'status' || payload.status !== 'completed') return false;
  return ['content', 'response', 'message'].some((key) => typeof payload[key] === 'string' && (payload[key] as string).trim().length > 0);
}
function Shell() {
  const [open, setOpen] = useState(false);
  const location = useLocation();
  const closeRail = useCallback(() => setOpen(false), []);
  useEffect(() => { requestAnimationFrame(() => document.getElementById('main-content')?.focus({ preventScroll: true })); }, [location.pathname]);
  useEffect(() => { try { const saved = JSON.parse(localStorage.getItem('chatcmd.preferences') ?? '{}') as { theme?: string }; if (saved.theme) document.documentElement.dataset.theme = saved.theme; } catch { /* invalid local preference */ } }, []);
  return <div className="shell"><a className="skip-link" href="#main-content">{tr('Skip to content')}</a><TaskRail open={open} onClose={closeRail} />{open && <button className="scrim" aria-label={tr('Close navigation')} onClick={closeRail} />}<div className="content-shell"><header className="mobile-topbar"><button className="icon-button" aria-label={tr('Open navigation')} onClick={() => setOpen(true)}><Menu /></button><strong>ChatCMD</strong><span>{tr('Local')}</span></header><main id="main-content" className={location.pathname.startsWith('/tasks') ? 'tasks-main' : undefined} tabIndex={-1}><Outlet /></main></div></div>;
}
function NotFound() { return <div className="state-panel"><Sparkles /><strong>{tr('Page not found')}</strong><span>{tr('This local route does not exist.')}</span><NavLink className="button primary" to="/">{tr('Open overview')}</NavLink></div>; }
