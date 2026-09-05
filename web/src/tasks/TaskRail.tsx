import { AlertTriangle, Bot, ChevronDown, ChevronUp, FolderOpen, LayoutDashboard, LoaderCircle, PanelLeftClose, PanelLeftOpen, Pencil, Plus, Power, Search, Settings, TerminalSquare, Trash2, Wrench } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent as ReactDragEvent, type MouseEventHandler } from 'react';
import { Link, NavLink, useLocation, useNavigate } from 'react-router-dom';

import { api } from '../api';
import { Empty, ErrorState, Loading, Modal } from '../components';
import { tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { Task, TimelineEvent, WorkspaceProject } from '../types';
import { upsertTaskEvent } from './taskTimeline';
import { useResizableWidth } from './useResizableWidth';
import { groupTasksByWorkspaceProjects } from './workspaceProjects';

const READ_FINAL_COUNTS_KEY = 'chatcmd.tasks.readFinalCounts.v1';
const PAGE_SIZE = 50;
const COLLAPSED_PROJECT_TASKS = 3;
const UNCLASSIFIED_GROUP = '__unclassified__';
const menuItems = [
  { to: '/', end: true, label: 'Overview', icon: LayoutDashboard },
  { to: '/sessions', label: 'Session', icon: TerminalSquare },
  { to: '/agents', label: 'Agents', icon: Bot },
  { to: '/skills', label: 'Skills', icon: Wrench },
  { to: '/settings', label: 'Setting', icon: Settings },
];

export function FunctionRail({ taskRailCollapsed, onTaskRailToggle }: { taskRailCollapsed: boolean; onTaskRailToggle: () => void }) {
  const navigate = useNavigate();
  const [confirmExit, setConfirmExit] = useState(false);
  const [exiting, setExiting] = useState(false);
  const [exitError, setExitError] = useState('');
  useEffect(() => {
    const openLogs = () => navigate('/settings?tab=data&section=extension');
    window.addEventListener('chatcmd:open-extension-logs', openLogs);
    return () => window.removeEventListener('chatcmd:open-extension-logs', openLogs);
  }, [navigate]);
  const exitApplication = async () => {
    if (exiting) return;
    setExiting(true); setExitError('');
    try { await api.exitApplication(); }
    catch (reason) { setExitError(reason instanceof Error ? reason.message : 'Không thể dừng ứng dụng.'); setExiting(false); }
  };
  return <>
    <nav className="function-rail" aria-label={tr('Application navigation')}>
      <Link className="function-rail-brand" to="/" aria-label="ChatCMD"><img src="/icons/logo-icon-master-1024.png" alt="" /></Link>
      {taskRailCollapsed && <button className="function-rail-action task-rail-reopen" type="button" aria-label="Mở rails đoạn trò chuyện" title="Mở rails đoạn trò chuyện" onClick={onTaskRailToggle}><PanelLeftOpen /><span className="sr-only">Mở rails đoạn trò chuyện</span></button>}
      <div className="function-rail-items">
        {menuItems.map(({ to, end, label, icon: Icon }) => <NavLink to={to} end={end} key={to} aria-label={tr(label)} title={tr(label)}><Icon /><span className="sr-only">{tr(label)}</span></NavLink>)}
        <button className="function-rail-action function-rail-exit" type="button" aria-label="Dừng ứng dụng" title="Dừng ứng dụng" onClick={() => { setExitError(''); setConfirmExit(true); }}><Power /><span className="sr-only">Dừng ứng dụng</span></button>
      </div>
    </nav>
    {confirmExit && <Modal title="Bạn có chắc chắn muốn dừng chạy ứng dụng không?" close={() => !exiting && setConfirmExit(false)} dangerous><div className="task-delete-warning"><AlertTriangle /><div><strong>Dừng ChatCMD</strong><p>Ứng dụng local sẽ được đóng ngay sau khi bạn xác nhận.</p></div></div>{exitError && <p className="task-delete-error" role="alert">{exitError}</p>}<div className="modal-actions"><button className="button secondary" type="button" disabled={exiting} onClick={() => setConfirmExit(false)}>Hủy</button><button className="button danger" type="button" disabled={exiting} onClick={() => void exitApplication()}>{exiting ? 'Đang dừng…' : 'Dừng ứng dụng'}</button></div></Modal>}
  </>;
}

export function TaskRail({ open, onClose, onDesktopCollapse }: { open: boolean; onClose: () => void; onDesktopCollapse: () => void }) {
  const location = useLocation(); const navigate = useNavigate(); const taskId = activeTaskId(location.pathname);
  const [loadedTasks, setLoadedTasks] = useState<Task[]>([]); const [nextCursor, setNextCursor] = useState<string>(); const [loading, setLoading] = useState(true); const [loadingMore, setLoadingMore] = useState(false); const [error, setError] = useState(''); const [query, setQuery] = useState(''); const [contextMenu, setContextMenu] = useState<{ task: Task; x: number; y: number }>(); const [deleteTarget, setDeleteTarget] = useState<Task>(); const [deleting, setDeleting] = useState(false); const [deleteError, setDeleteError] = useState('');
  const [readFinalCounts, setReadFinalCounts] = useState<Record<string, number>>(readStoredFinalCounts);
  const [projects, setProjects] = useState<WorkspaceProject[]>([]);
  const [draggedProjectId, setDraggedProjectId] = useState<string>(); const [dragOverProjectId, setDragOverProjectId] = useState<string>();
  const [expandedGroupKeys, setExpandedGroupKeys] = useState<Set<string>>(() => new Set([UNCLASSIFIED_GROUP]));
  const [visibleGroupCounts, setVisibleGroupCounts] = useState<Record<string, number>>({});
  const [projectHasMore, setProjectHasMore] = useState<Record<string, boolean>>({});
  const [loadingProjectMore, setLoadingProjectMore] = useState<Record<string, boolean>>({});
  const [projectModalOpen, setProjectModalOpen] = useState(false); const [editingProject, setEditingProject] = useState<WorkspaceProject>(); const [projectName, setProjectName] = useState(''); const [projectPath, setProjectPath] = useState(''); const [projectFolderPicking, setProjectFolderPicking] = useState(false); const [projectSaving, setProjectSaving] = useState(false); const [projectError, setProjectError] = useState('');
  const [projectContextMenu, setProjectContextMenu] = useState<{ project: WorkspaceProject; x: number; y: number }>(); const [deleteProjectTarget, setDeleteProjectTarget] = useState<WorkspaceProject>(); const [deletingProject, setDeletingProject] = useState(false); const [deleteProjectError, setDeleteProjectError] = useState('');
  const visibleTaskIds = useRef(new Set<string>()); const loadingMoreRef = useRef(false); const groupExpansionInitialized = useRef(false); const hadStoredReadCounts = useRef(typeof localStorage !== 'undefined' && localStorage.getItem(READ_FINAL_COUNTS_KEY) !== null);
  const railResize = useResizableWidth({ storageKey: 'chatcmd.layout.taskRailWidth.v1', cssVariable: '--task-rail-width', defaultWidth: typeof window !== 'undefined' && window.innerWidth <= 1180 ? 270 : 284, minWidth: 240, maxWidth: 480 });

  const applyFirstPage = useCallback(async () => {
    setLoading(true); setError('');
    try { const [page, workspaceProjects] = await Promise.all([api.tasks(undefined, PAGE_SIZE), api.workspaceProjects()]); setLoadedTasks(pageItems(page).filter((task) => !task.isSubagent)); setNextCursor(pageCursor(page)); setProjects(workspaceProjects); }
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
  useEffect(() => {
    const cleared = () => { setLoadedTasks([]); setNextCursor(undefined); setReadFinalCounts({}); void applyFirstPage(); };
    window.addEventListener('chatcmd:conversations-cleared', cleared);
    return () => window.removeEventListener('chatcmd:conversations-cleared', cleared);
  }, [applyFirstPage]);
  useEffect(() => { visibleTaskIds.current = new Set(loadedTasks.map((task) => task.id)); }, [loadedTasks]);
  useEffect(() => { if (loading || hadStoredReadCounts.current) return; hadStoredReadCounts.current = true; setReadFinalCounts(Object.fromEntries(loadedTasks.map((task) => [task.id, task.finalResponseCount ?? 0]))); }, [loadedTasks, loading]);
  const handleRealtime = useCallback((event: TimelineEvent) => { if (event.type === 'system.connected') { void refreshHead(); return; } if (!event.taskId) return; if (visibleTaskIds.current.has(event.taskId)) setLoadedTasks((current) => upsertTaskEvent(current, event) ?? current); else void refreshHead(); }, [refreshHead]);
  useRealtime(handleRealtime);
  useEffect(() => { try { localStorage.setItem(READ_FINAL_COUNTS_KEY, JSON.stringify(readFinalCounts)); } catch { /* unavailable */ } }, [readFinalCounts]);
  useEffect(() => { if (!taskId) return; const task = loadedTasks.find((item) => item.id === taskId); if (!task) return; const count = task.finalResponseCount ?? 0; setReadFinalCounts((current) => (current[taskId] ?? 0) >= count ? current : { ...current, [taskId]: count }); }, [taskId, loadedTasks]);
  useEffect(() => { setContextMenu(undefined); setProjectContextMenu(undefined); onClose(); }, [location.pathname, onClose]);
  useEffect(() => { if (!contextMenu && !projectContextMenu) return; const close = () => { setContextMenu(undefined); setProjectContextMenu(undefined); }; window.addEventListener('pointerdown', close); window.addEventListener('blur', close); return () => { window.removeEventListener('pointerdown', close); window.removeEventListener('blur', close); }; }, [contextMenu, projectContextMenu]);
  const deleteConversation = useCallback(async () => {
    if (!deleteTarget || !canDeleteTask(deleteTarget)) return; setDeleting(true); setDeleteError('');
    try { await api.deleteTask(deleteTarget.id); setLoadedTasks((current) => current.filter((task) => task.id !== deleteTarget.id)); setReadFinalCounts((current) => { const next = { ...current }; delete next[deleteTarget.id]; return next; }); if (taskId === deleteTarget.id) navigate('/tasks'); setDeleteTarget(undefined); }
    catch (value) { setDeleteError(value instanceof Error ? value.message : tr('Could not delete conversation.')); }
    finally { setDeleting(false); }
  }, [deleteTarget, navigate, taskId]);

  const tasks = useMemo(() => [...loadedTasks].sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc)).filter((task) => `${conversationName(task)} ${task.id} ${task.outputPreview ?? ''}`.toLowerCase().includes(query.toLowerCase())), [query, loadedTasks]);
  const taskGroups = useMemo(() => groupTasksByWorkspaceProjects(projects, tasks), [projects, tasks]);
  useEffect(() => {
    if (loading || groupExpansionInitialized.current) return;
    groupExpansionInitialized.current = true;
    const grouped = groupTasksByWorkspaceProjects(projects, [...loadedTasks].sort((a, b) => Date.parse(b.updatedAtUtc) - Date.parse(a.updatedAtUtc)));
    const expanded = new Set<string>([UNCLASSIFIED_GROUP]);
    if (grouped.projects[0]) expanded.add(grouped.projects[0].project.id);
    for (const { project, tasks: projectTasks } of grouped.projects) {
      if (projectTasks.some((task) => task.status === 'running' || task.id === taskId)) expanded.add(project.id);
    }
    setExpandedGroupKeys(expanded);
  }, [loadedTasks, loading, projects, taskId]);
  useEffect(() => {
    const forcedProjectKeys = taskGroups.projects
      .filter(({ tasks: projectTasks }) => projectTasks.some((task) => task.status === 'running' || task.id === taskId))
      .map(({ project }) => project.id);
    if (!forcedProjectKeys.length) return;
    setExpandedGroupKeys((current) => {
      const next = new Set(current); let changed = false;
      for (const key of forcedProjectKeys) if (!next.has(key)) { next.add(key); changed = true; }
      return changed ? next : current;
    });
  }, [taskGroups, taskId]);

  const reorderProjects = async (sourceId: string, targetId: string) => {
    if (sourceId === targetId) return;
    const previous = [...projects];
    const sourceIndex = previous.findIndex((project) => project.id === sourceId); const targetIndex = previous.findIndex((project) => project.id === targetId);
    if (sourceIndex < 0 || targetIndex < 0) return;
    const next = [...previous]; const [moved] = next.splice(sourceIndex, 1); next.splice(targetIndex, 0, moved);
    setProjects(next); setDragOverProjectId(undefined);
    try { await api.reorderWorkspaceProjects(next.map((project) => project.id)); }
    catch (value) { setProjects(previous); setError(value instanceof Error ? value.message : 'Không thể lưu thứ tự dự án.'); }
  };
  const startTask = (project?: WorkspaceProject) => navigate('/tasks/new', { state: project ? { projectFolder: project.path, projectName: project.name } : undefined });
  const openProjectModal = (project?: WorkspaceProject) => { setEditingProject(project); setProjectName(project?.name ?? ''); setProjectPath(project?.path ?? ''); setProjectError(''); setProjectModalOpen(true); };
  const pickProjectFolder = async () => {
    if (projectFolderPicking) return;
    setProjectFolderPicking(true); setProjectError('');
    try { const result = await api.pickProjectFolder(); if (result.path) setProjectPath(result.path); }
    catch (reason) { setProjectError(reason instanceof Error ? reason.message : 'Không thể mở trình chọn thư mục.'); }
    finally { setProjectFolderPicking(false); }
  };
  const saveProject = async () => {
    if (!projectName.trim() || !projectPath.trim()) { setProjectError('Vui lòng nhập tên và chọn thư mục dự án.'); return; }
    setProjectSaving(true); setProjectError('');
    try {
      const input = { name: projectName.trim(), path: projectPath.trim() };
      if (editingProject) await api.updateWorkspaceProject(editingProject.id, input); else await api.saveWorkspaceProject(input);
      setProjects(await api.workspaceProjects()); setProjectModalOpen(false); setEditingProject(undefined);
    }
    catch (reason) { setProjectError(reason instanceof Error ? reason.message : 'Không thể lưu dự án.'); }
    finally { setProjectSaving(false); }
  };
  const deleteProject = async () => {
    if (!deleteProjectTarget || deletingProject) return;
    setDeletingProject(true); setDeleteProjectError('');
    try {
      await api.deleteWorkspaceProject(deleteProjectTarget.id);
      setDeleteProjectTarget(undefined);
      await applyFirstPage();
    } catch (reason) { setDeleteProjectError(reason instanceof Error ? reason.message : 'Không thể xóa dự án.'); }
    finally { setDeletingProject(false); }
  };
  const renderRow = (task: Task) => <TaskRailRow task={task} selected={task.id === taskId} unread={Math.max(0, (task.finalResponseCount ?? 0) - (readFinalCounts[task.id] ?? 0))} onRenamed={(updated) => setLoadedTasks((current) => current.map((item) => item.id === updated.id ? { ...item, ...updated } : item))} onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); setProjectContextMenu(undefined); setContextMenu({ task, x: Math.min(event.clientX, window.innerWidth - 236), y: Math.min(event.clientY, window.innerHeight - 108) }); }} key={task.id} />;
  const loadMoreProject = async (key: string, project: WorkspaceProject, visible: Task[]) => {
    if (loadingProjectMore[key] || !visible.length) return;
    setLoadingProjectMore((current) => ({ ...current, [key]: true }));
    try {
      const page = await api.tasks(visible.at(-1)?.id, COLLAPSED_PROJECT_TASKS, project.path);
      setLoadedTasks((current) => mergeTasks(current, pageItems(page).filter((task) => !task.isSubagent)));
      setProjectHasMore((current) => ({ ...current, [key]: Boolean(pageCursor(page)) }));
      setVisibleGroupCounts((current) => ({ ...current, [key]: visible.length + COLLAPSED_PROJECT_TASKS }));
    } catch (value) { setError(value instanceof Error ? value.message : tr('Could not load more conversations.')); }
    finally { setLoadingProjectMore((current) => ({ ...current, [key]: false })); }
  };
  const renderGroup = (key: string, name: string, groupTasks: Task[], project?: WorkspaceProject) => {
    const expanded = expandedGroupKeys.has(key);
    const visibleCount = visibleGroupCounts[key] ?? COLLAPSED_PROJECT_TASKS;
    const visible = groupTasks.slice(0, visibleCount);
    const canLoadProjectMore = Boolean(project && groupTasks.length >= visibleCount && projectHasMore[key] !== false);
    const canShowMore = groupTasks.length > visibleCount || canLoadProjectMore;
    const toggleExpanded = () => setExpandedGroupKeys((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
    const dragClass = project ? `${draggedProjectId === key ? ' dragging' : ''}${dragOverProjectId === key && draggedProjectId !== key ? ' drag-over' : ''}` : '';
    const handleDragOver = project ? (event: ReactDragEvent<HTMLElement>) => { event.preventDefault(); if (!draggedProjectId || draggedProjectId === project.id) return; event.dataTransfer.dropEffect = 'move'; setDragOverProjectId(project.id); } : undefined;
    const handleDrop = project ? (event: ReactDragEvent<HTMLElement>) => { event.preventDefault(); const sourceId = draggedProjectId || event.dataTransfer.getData('text/plain'); setDraggedProjectId(undefined); setDragOverProjectId(undefined); if (sourceId) void reorderProjects(sourceId, project.id); } : undefined;
    return <section className={`task-project-group ${expanded ? 'expanded' : 'collapsed'}${dragClass}`} key={key} onDragOver={handleDragOver} onDrop={handleDrop} onContextMenu={project ? (event) => { event.preventDefault(); setContextMenu(undefined); setProjectContextMenu({ project, x: Math.min(event.clientX, window.innerWidth - 236), y: Math.min(event.clientY, window.innerHeight - 112) }); } : undefined}>
      <header className="task-project-heading"><button className="task-project-toggle" type="button" onClick={toggleExpanded} aria-expanded={expanded} aria-label={`${expanded ? 'Ẩn' : 'Hiện'} đoạn trò chuyện của ${name}`}><ChevronDown /><span className={project ? 'task-project-title-drag-handle' : undefined} draggable={Boolean(project)} title={project ? `Kéo để sắp xếp ${name}` : undefined} onDragStart={project ? (event) => { setDraggedProjectId(project.id); setDragOverProjectId(undefined); event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', project.id); } : undefined} onDragEnd={project ? () => { setDraggedProjectId(undefined); setDragOverProjectId(undefined); } : undefined}><strong title={project?.path}>{name}</strong>{project && <small title={project.path}>{project.path}</small>}</span></button><button className="task-project-add" type="button" onClick={() => startTask(project)} aria-label={`Tạo đoạn trò chuyện trong ${name}`} title={`Tạo đoạn trò chuyện trong ${name}`}><Plus /></button></header>
      {expanded && <><div className="task-project-conversations">{visible.length ? visible.map(renderRow) : <p className="task-project-empty">Chưa có đoạn trò chuyện</p>}</div>
      {(canShowMore || visibleCount > COLLAPSED_PROJECT_TASKS) && <div className="task-project-more-actions">{canShowMore && <button className="task-project-more" type="button" disabled={Boolean(loadingProjectMore[key])} onClick={() => project ? void loadMoreProject(key, project, visible) : setVisibleGroupCounts((current) => ({ ...current, [key]: visibleCount + COLLAPSED_PROJECT_TASKS }))}>{loadingProjectMore[key] ? <LoaderCircle className="spin" /> : <ChevronDown />}Xem thêm</button>}{visibleCount > COLLAPSED_PROJECT_TASKS && <><span aria-hidden="true">|</span><button className="task-project-more" type="button" onClick={() => setVisibleGroupCounts((current) => ({ ...current, [key]: COLLAPSED_PROJECT_TASKS }))}><ChevronUp />Ẩn bớt</button></>}</div>}</>}
    </section>;
  };

  return <aside className={`task-rail ${open ? 'open' : ''}`} aria-label={tr('Conversations')}>
    <div className="panel-resize-handle task-rail-resize-handle" role="separator" aria-label={tr('Resize conversations')} aria-orientation="vertical" aria-valuemin={240} aria-valuemax={480} aria-valuenow={railResize.width} tabIndex={0} onPointerDown={railResize.onPointerDown} onKeyDown={railResize.onKeyDown} />
    <header className="task-rail-header">
      <div className="task-rail-toolbar">
        <button className="task-rail-collapse" type="button" aria-label="Đóng rails đoạn trò chuyện" title="Đóng rails đoạn trò chuyện" onClick={onDesktopCollapse}><PanelLeftClose /></button>
        <label className="tasks-conversation-search"><Search /><span className="sr-only">{tr('Search conversations')}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={tr('Search')} /></label>
        <Link className="task-rail-new-message" to="/tasks/new" aria-label={tr('New message')} title={tr('New message')}><Plus /></Link>
      </div>
      <div className="task-projects-title"><strong>Dự án</strong><button type="button" onClick={() => openProjectModal()} aria-label="Thêm dự án" title="Thêm dự án"><Plus /></button></div>
    </header>
    <div className="task-rail-body"><div className="task-rail-list" onScroll={(event) => { const target = event.currentTarget; if (target.scrollHeight - target.scrollTop - target.clientHeight < 180) void loadMore(); }}>
      {loading ? <Loading label={tr('Loading tasks')} /> : error && !tasks.length ? <ErrorState message={error} retry={() => void applyFirstPage()} /> : <>
        {taskGroups.projects.map(({ project, tasks: projectTasks }) => renderGroup(project.id, project.name, projectTasks, project))}
        {renderGroup(UNCLASSIFIED_GROUP, 'Chưa phân loại', taskGroups.unclassified)}
        {!projects.length && !taskGroups.unclassified.length && <Empty title={tr('No conversations yet')} body="Thêm dự án hoặc tạo đoạn trò chuyện mới để bắt đầu." />}
        {loadingMore && <div className="task-rail-load-more" role="status"><LoaderCircle className="spin" /><span>{tr('Loading more…')}</span></div>}
        {nextCursor && !loadingMore && <button className="task-rail-load-retry" type="button" onClick={() => void loadMore()}>Tải thêm đoạn trò chuyện</button>}
        {error && <button className="task-rail-load-retry" type="button" onClick={() => void loadMore()}>{tr('Reload')}</button>}
      </>}
    </div></div>
    {contextMenu && <div className="task-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }} onPointerDown={(event) => event.stopPropagation()}><button type="button" role="menuitem" className="danger" disabled={!canDeleteTask(contextMenu.task)} onClick={() => { setDeleteError(''); setDeleteTarget(contextMenu.task); setContextMenu(undefined); }}><Trash2 /><span>{tr('Delete conversation')}</span></button>{!canDeleteTask(contextMenu.task) && <small>{tr('You can only delete a task after it has finished.')}</small>}</div>}
    {projectContextMenu && <div className="task-context-menu" role="menu" style={{ left: projectContextMenu.x, top: projectContextMenu.y }} onPointerDown={(event) => event.stopPropagation()}><button type="button" role="menuitem" onClick={() => { const project = projectContextMenu.project; setProjectContextMenu(undefined); openProjectModal(project); }}><Pencil /><span>Sửa dự án</span></button><button type="button" role="menuitem" className="danger" onClick={() => { setDeleteProjectError(''); setDeleteProjectTarget(projectContextMenu.project); setProjectContextMenu(undefined); }}><Trash2 /><span>Xóa dự án</span></button></div>}
    {deleteTarget && <Modal title={tr('Delete conversation?')} description={conversationName(deleteTarget)} close={() => !deleting && setDeleteTarget(undefined)} dangerous><div className="task-delete-warning"><AlertTriangle /><div><strong>{tr('Warning')}</strong><p>{tr('Deleting removes this conversation and its linked data from the list. This conversation may not work again in the future.')}</p></div></div>{deleteError && <p className="task-delete-error" role="alert">{deleteError}</p>}<div className="modal-actions"><button className="button secondary" type="button" disabled={deleting} onClick={() => setDeleteTarget(undefined)}>{tr('Cancel')}</button><button className="button danger" type="button" disabled={deleting} onClick={() => void deleteConversation()}>{deleting ? tr('Deleting…') : tr('Delete conversation')}</button></div></Modal>}
    {deleteProjectTarget && <Modal title="Xóa dự án?" description={deleteProjectTarget.name} close={() => !deletingProject && setDeleteProjectTarget(undefined)} dangerous><div className="task-delete-warning"><AlertTriangle /><div><strong>Toàn bộ dự án sẽ bị xóa</strong><p>Các cuộc trò chuyện đã hoàn tất trong dự án cũng sẽ bị xóa. Cuộc trò chuyện chưa hoàn tất sẽ được giữ lại và chuyển vào mục “Chưa phân loại”.</p></div></div>{deleteProjectError && <p className="task-delete-error" role="alert">{deleteProjectError}</p>}<div className="modal-actions"><button className="button secondary" type="button" disabled={deletingProject} onClick={() => setDeleteProjectTarget(undefined)}>Hủy</button><button className="button danger" type="button" disabled={deletingProject} onClick={() => void deleteProject()}>{deletingProject ? 'Đang xóa…' : 'Xóa dự án'}</button></div></Modal>}
    {projectModalOpen && <Modal className="workspace-project-modal" title={editingProject ? 'Sửa dự án' : 'Thêm dự án'} description={editingProject ? 'Cập nhật tên hiển thị hoặc thư mục gốc của dự án.' : 'Lưu tên hiển thị và thư mục gốc để nhóm các đoạn trò chuyện theo dự án.'} close={() => { if (!projectFolderPicking && !projectSaving) { setProjectModalOpen(false); setEditingProject(undefined); } }}><div className="workspace-project-form"><label><span>Tên</span><input value={projectName} onChange={(event) => setProjectName(event.target.value)} placeholder="Ví dụ: Dotty" autoFocus maxLength={160} disabled={projectSaving} /></label><label><span>Thư mục dự án</span><button className={`workspace-project-folder ${projectPath ? '' : 'empty'}`} type="button" onClick={() => void pickProjectFolder()} disabled={projectFolderPicking || projectSaving}>{projectFolderPicking ? <LoaderCircle className="spin" /> : <FolderOpen />}<span>{projectPath || 'Chọn folder'}</span></button></label>{projectError && <p className="workspace-project-error" role="alert">{projectError}</p>}<div className="modal-actions"><button className="button secondary" type="button" onClick={() => { setProjectModalOpen(false); setEditingProject(undefined); }} disabled={projectFolderPicking || projectSaving}>Hủy</button><button className="button primary" type="button" onClick={() => void saveProject()} disabled={projectFolderPicking || projectSaving || !projectName.trim() || !projectPath.trim()}>{projectSaving ? 'Đang lưu…' : editingProject ? 'Lưu thay đổi' : 'Lưu'}</button></div></div></Modal>}
  </aside>;
}

function TaskRailRow({ task, selected, unread, onRenamed, onContextMenu }: { task: Task; selected: boolean; unread: number; onRenamed: (task: Task) => void; onContextMenu: MouseEventHandler<HTMLAnchorElement> }) {
  const running = task.status === 'running';
  const [editing, setEditing] = useState(false); const [title, setTitle] = useState(conversationName(task)); const [saving, setSaving] = useState(false); const [renameError, setRenameError] = useState('');
  useEffect(() => { if (!editing) setTitle(conversationName(task)); }, [editing, task]);
  const saveTitle = async () => {
    const next = title.trim(); if (!next || saving) return;
    setSaving(true); setRenameError('');
    try { const result = await api.setTaskTitle(task.id, next); onRenamed(result.task); setEditing(false); }
    catch (value) { setRenameError(value instanceof Error ? value.message : tr('Could not rename conversation.')); }
    finally { setSaving(false); }
  };
  const createdLabel = task.status === 'completed' ? formatConversationCreatedAt(task.createdAtUtc ?? task.updatedAtUtc) : undefined;
  const fromChatCmd = task.source === 'chatgpt_web';
  return <Link className={`tasks-conversation-row ${selected ? 'selected' : ''} ${unread > 0 ? 'unread' : ''}`} aria-current={selected ? 'page' : undefined} to={`/tasks/${encodeURIComponent(task.id)}`} onContextMenu={onContextMenu}><span className="tasks-conversation-copy"><span className="tasks-conversation-title-row">{!editing && fromChatCmd && <span className="task-rail-origin-icon" title="Mở từ ChatCMD" aria-label="Mở từ ChatCMD"><img src="/icons/logo-icon-master-1024.png" alt="" /></span>}{editing ? <input className="task-rail-title-input" value={title} maxLength={160} autoFocus disabled={saving} aria-label={tr('Conversation title')} onClick={(event) => { event.preventDefault(); event.stopPropagation(); }} onBlur={() => { if (!saving) { setEditing(false); setRenameError(''); } }} onChange={(event) => setTitle(event.target.value)} onKeyDown={(event) => { event.stopPropagation(); if (event.key === 'Enter') { event.preventDefault(); void saveTitle(); } else if (event.key === 'Escape') { event.preventDefault(); setTitle(conversationName(task)); setRenameError(''); setEditing(false); } }} /> : <strong title={selected ? tr('Click again to rename') : undefined} onClick={(event) => { if (!selected) return; event.preventDefault(); event.stopPropagation(); setTitle(conversationName(task)); setRenameError(''); setEditing(true); }}>{conversationName(task)}</strong>}{unread > 0 && !editing && <span className="task-unread-badge" aria-label={tr('{count} unread final responses', { count: unread })}>{unread > 99 ? '99+' : unread}</span>}</span>{renameError && <small className="task-rail-rename-error">{renameError}</small>}<span className="tasks-conversation-status-line"><span className={`task-rail-state ${task.status}`}>{running ? <LoaderCircle className="spin" /> : <i />}</span><span>{taskStatusLabel(task.status)}{createdLabel ? ` · ${createdLabel}` : ''}</span></span></span></Link>;
}
function pageItems(page: { items?: Task[] } | Task[]) { return Array.isArray(page) ? page : page.items ?? []; }
function pageCursor(page: { nextCursor?: string } | Task[]) { return Array.isArray(page) ? undefined : page.nextCursor; }
export function mergeTasks(first: Task[], second: Task[]) {
  const merged = new Map<string, Task>();
  for (const task of [...first, ...second]) {
    const current = merged.get(task.id);
    if (!current) { merged.set(task.id, task); continue; }
    const currentTime = Date.parse(current.updatedAtUtc) || 0;
    const nextTime = Date.parse(task.updatedAtUtc) || 0;
    if (nextTime > currentTime) merged.set(task.id, task);
  }
  return [...merged.values()];
}
function canDeleteTask(task: Task) { return ['completed', 'failed', 'stopped', 'interrupted'].includes(task.status); }
function taskStatusLabel(status: string) { if (status === 'running') return tr('Processing'); if (status === 'completed') return tr('Complete'); if (status === 'failed') return tr('Has errors'); if (status === 'stopped') return tr('Stopped'); return status; }
function formatConversationCreatedAt(value: string) {
  const createdAt = new Date(value);
  if (Number.isNaN(createdAt.getTime())) return '';
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const createdDay = new Date(createdAt.getFullYear(), createdAt.getMonth(), createdAt.getDate());
  const dayDiff = Math.round((Date.UTC(today.getFullYear(), today.getMonth(), today.getDate()) - Date.UTC(createdDay.getFullYear(), createdDay.getMonth(), createdDay.getDate())) / 86_400_000);
  if (dayDiff === 0) return createdAt.toLocaleTimeString('vi-VN', { hour: '2-digit', minute: '2-digit', hour12: false });
  if (dayDiff === 1) return 'Hôm qua';
  if (createdAt.getFullYear() === now.getFullYear()) return createdAt.toLocaleDateString('vi-VN', { day: '2-digit', month: '2-digit' });
  return createdAt.toLocaleDateString('vi-VN', { day: '2-digit', month: '2-digit', year: 'numeric' });
}
function activeTaskId(pathname: string) { if (!pathname.startsWith('/tasks/')) return undefined; const value = pathname.slice('/tasks/'.length).split('/')[0]; if (!value || value === 'new') return undefined; try { return decodeURIComponent(value); } catch { return value; } }
function readStoredFinalCounts(): Record<string, number> { try { const value = JSON.parse(localStorage.getItem(READ_FINAL_COUNTS_KEY) ?? '{}') as Record<string, unknown>; return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] >= 0)); } catch { return {}; } }
function conversationName(task: Task) { return task.title?.trim() || task.agentName?.trim() || generatedConversationName(task.id); }
function generatedConversationName(id: string) { const first = [tr('Cloud'), tr('Star'), tr('Wind'), tr('Sun'), tr('Moon'), tr('Sea'), tr('Forest'), tr('Mist')]; const second = [tr('Blue'), tr('Soft'), tr('Morning'), tr('Night'), tr('New'), tr('Far'), tr('Warm'), tr('Bright')]; let hash = 2166136261; for (let index = 0; index < id.length; index++) { hash ^= id.charCodeAt(index); hash = Math.imul(hash, 16777619); } const value = hash >>> 0; return `${first[value % first.length]} ${second[Math.floor(value / first.length) % second.length]} ${String(value % 97 + 1).padStart(2, '0')}`; }
