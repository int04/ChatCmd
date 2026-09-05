import { Menu, ShieldAlert, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { NavLink, Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { api } from './api';
import { AuthGate } from './auth/AuthGate';
import { applyAppFont, applyTaskFontScale, DEFAULT_APP_FONT, DEFAULT_TASK_FONT_SCALE } from './fontPreferences';
import { useAppLanguage, tr } from './i18n';
import { AgentsPage } from './pages/AgentsPage';
import { DashboardPage } from './pages/DashboardPage';
import { LiveTerminalPage } from './pages/LiveTerminalPage';
import { SessionsPage, SessionDetailPage } from './pages/SessionsPage';
import { SettingsPage } from './pages/SettingsPage';
import { SkillsPage } from './pages/SkillsPage';
import { TasksPage } from './pages/TasksPage';
import { RealtimeProvider, useRealtime } from './realtime';
import { soundNotifications } from './soundNotifications';
import { GlobalConversationApprovalQueue } from './tasks/GlobalConversationApprovalQueue';
import { GlobalPlanQuestionQueue } from './tasks/GlobalPlanQuestionQueue';
import { GlobalSubagentFallbackBridge } from './tasks/GlobalSubagentFallbackBridge';
import { FunctionRail, TaskRail } from './tasks/TaskRail';
import { useTaskDocumentTitle } from './tasks/taskDocumentTitle';
import type { TimelineEvent } from './types';

const legacyPaths = ['/login', '/register', '/plans', '/account', '/payment', '/payments', '/purchase', '/checkout'];

export default function App() {
  useAppLanguage();
  return <AuthGate><AdminElevationPrompt /><RealtimeProvider><GlobalDocumentTitleBridge /><SoundNotificationsBridge /><GlobalSubagentFallbackBridge /><GlobalConversationApprovalQueue /><GlobalPlanQuestionQueue /><Routes><Route element={<Shell />}><Route index element={<DashboardPage />} /><Route path="tasks/:taskId?" element={<TasksPage />} /><Route path="sessions" element={<SessionsPage />} /><Route path="sessions/terminal/:sessionId" element={<LiveTerminalPage />} /><Route path="sessions/:sessionId" element={<SessionDetailPage />} /><Route path="agents" element={<AgentsPage />} /><Route path="skills" element={<SkillsPage />} /><Route path="settings" element={<SettingsPage />} />{legacyPaths.map((path) => <Route key={path} path={path} element={<Navigate replace to="/" />} />)}<Route path="*" element={<NotFound />} /></Route></Routes></RealtimeProvider></AuthGate>;
}

function AdminElevationPrompt() {
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    void api.elevationStatus()
      .then((status) => { if (active) setVisible(status.supported && !status.elevated); })
      .catch(() => undefined);
    return () => { active = false; };
  }, []);

  if (!visible) return null;
  const restart = async () => {
    setBusy(true);
    setError('');
    try {
      await api.restartElevated();
    } catch (reason) {
      setBusy(false);
      setError(reason instanceof Error ? reason.message : tr('Unable to restart ChatCMD as administrator.'));
    }
  };

  return <div className="admin-elevation-banner" role="status"><ShieldAlert /><div><strong>{tr('Run ChatCMD as administrator')}</strong><span>{tr('Administrator access is recommended so local tools can run with full system permissions.')}</span>{error && <span className="admin-elevation-error">{error}</span>}</div><div className="admin-elevation-actions"><button className="button secondary" type="button" disabled={busy} onClick={() => setVisible(false)}>{tr('Close')}</button><button className="button primary" type="button" disabled={busy} onClick={() => void restart()}>{busy ? tr('Restarting…') : tr('Run as administrator')}</button></div></div>;
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
  const [taskRailCollapsed, setTaskRailCollapsed] = useState(() => localStorage.getItem('chatcmd.layout.taskRailCollapsed.v1') === 'true');
  const location = useLocation();
  const previousPath = useRef(location.pathname);
  const closeRail = useCallback(() => setOpen(false), []);
  const toggleTaskRail = useCallback(() => setTaskRailCollapsed((current) => {
    const next = !current;
    localStorage.setItem('chatcmd.layout.taskRailCollapsed.v1', String(next));
    return next;
  }), []);
  useEffect(() => {
    if (previousPath.current === location.pathname) return;
    previousPath.current = location.pathname;
    requestAnimationFrame(() => document.getElementById('main-content')?.focus({ preventScroll: true }));
  }, [location.pathname]);
  useEffect(() => { try { const saved = JSON.parse(localStorage.getItem('chatcmd.preferences') ?? '{}') as { theme?: string; fontFamily?: string; taskFontScale?: number }; document.documentElement.dataset.theme = saved.theme ?? 'dark'; applyAppFont(saved.fontFamily ?? DEFAULT_APP_FONT); applyTaskFontScale(saved.taskFontScale ?? DEFAULT_TASK_FONT_SCALE); } catch { document.documentElement.dataset.theme = 'dark'; applyAppFont(DEFAULT_APP_FONT); applyTaskFontScale(DEFAULT_TASK_FONT_SCALE); } }, []);
  return <div className={`shell${taskRailCollapsed ? ' task-rail-collapsed' : ''}`}><a className="skip-link" href="#main-content">{tr('Skip to content')}</a><FunctionRail taskRailCollapsed={taskRailCollapsed} onTaskRailToggle={toggleTaskRail} /><TaskRail open={open} onClose={closeRail} onDesktopCollapse={toggleTaskRail} />{open && <button className="scrim" aria-label={tr('Close navigation')} onClick={closeRail} />}<div className="content-shell"><header className="mobile-topbar"><button className="icon-button" aria-label={tr('Open navigation')} onClick={() => setOpen(true)}><Menu /></button><strong>ChatCMD</strong><span>{tr('Local')}</span></header><main id="main-content" className={location.pathname.startsWith('/tasks') ? 'tasks-main' : undefined} tabIndex={-1}><Outlet /></main></div></div>;
}

function NotFound() { return <div className="state-panel"><Sparkles /><strong>{tr('Page not found')}</strong><span>{tr('This local route does not exist.')}</span><NavLink className="button primary" to="/">{tr('Open overview')}</NavLink></div>; }
