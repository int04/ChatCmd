import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import type { ReactNode } from 'react';
import { TasksPage } from '../pages/TasksPage';
import { SettingsPage } from '../pages/SettingsPage';
import { api } from '../api';
import { setAppLanguage } from '../i18n';
import { subagentTreeRows } from '../tasks/subagentPresentation';
import type { SubagentRun, TimelineEvent } from '../types';
// Actual overflow/scroll geometry is verified in scripts/check-task-detail-layout.mjs.

const scene = vi.hoisted(() => ({ data: null as unknown, refresh: vi.fn(), setData: vi.fn(), listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../useLoad', () => ({ useLoad: () => ({ data: scene.data, loading: false, error: '', refresh: scene.refresh, setData: scene.setData }) }));
vi.mock('../realtime', () => ({ useRealtime: (callback: (event: TimelineEvent) => void) => { scene.listener = callback; return 'online'; } }));
vi.mock('../chatgpt/compact/CompactProvider', () => ({ CompactProvider: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('../chatgpt/compact/CompactHistoryCard', () => ({ CompactHistoryCard: () => null }));
vi.mock('../chatgpt/ChatGptConversation', () => ({ ChatGptTaskCard: () => null, ChatGptTaskComposer: () => <textarea aria-label="Message" />, NewChatGptConversation: () => null }));
vi.mock('../tasks/TaskAccessCard', () => ({ TaskAccessCard: () => null }));
vi.mock('../tasks/TaskTerminalSection', () => ({ TaskTerminalSection: () => null }));
vi.mock('../tasks/TaskTurnBubble', () => ({ TaskTurnBubble: ({ turn, subagents }: { turn: { id: string }; subagents: SubagentRun[] }) => <div data-testid="turn-children" data-turn-id={turn.id}>{subagents.map((v) => v.name).join(', ')}</div> }));

function agent(id: string, parentTaskId: string, parentTurnId = 'root-turn'): SubagentRun {
  return { id, name: id, taskId: `task-${id}`, parentTaskId, parentTurnId, rootTurnId: 'root-turn', request: '', status: 'running', attempt: 1, maxRuntimeMs: 1800000, createdAtUtc: '2026-09-06T00:00:00Z', updatedAtUtc: '2026-09-06T00:00:00Z' };
}
function chat() {
  scene.data = { task: { id: 'task-chat-root', source: 'chatgpt_web', title: 'Root conversation', status: 'running', updatedAtUtc: '2026-09-06T00:00:00Z', allowExecute: true }, turns: [{ id: 'root-turn', status: 'running' }], subagents: [agent('child', 'task-chat-root'), agent('grandchild', 'task-child', 'child-turn')] };
  return render(<MemoryRouter initialEntries={['/tasks/task-chat-root']}><Routes><Route path="/tasks/:taskId" element={<TasksPage />} /></Routes></MemoryRouter>);
}

beforeEach(() => { localStorage.clear(); setAppLanguage('en', false); scene.listener = null; });

describe('subagent tree and chat layout', () => {
  it('preserves depth, parent ordering and legacy orphan entries', () => {
    const rows = subagentTreeRows([agent('grandchild', 'task-child'), agent('child', 'root'), agent('orphan', 'missing')]);
    expect(rows.map(({ agent: value, depth }) => [value.id, depth])).toEqual([['child', 0], ['grandchild', 1], ['orphan', 0]]);
    expect(subagentTreeRows([agent('a', 'task-b'), agent('b', 'task-a')])).toHaveLength(2);
  });
  it('puts the focusable topbar inside the chat column, next to a full-height sidebar', () => {
    const { container } = chat();
    const topbar = screen.getByLabelText('Conversation header');
    const body = container.querySelector('.task-detail-body')!;
    expect(topbar.parentElement).toHaveClass('task-chat-pane');
    expect(topbar.parentElement?.parentElement).toBe(body);
    expect(body.querySelector(':scope > .task-detail-sidebar')).not.toBeNull();
    expect(topbar).toHaveAttribute('tabindex', '0');
    expect(topbar.nextElementSibling).toHaveClass('task-chat-column');
    expect(screen.getByTestId('turn-children')).toHaveTextContent('child, grandchild');
    fireEvent.click(screen.getByRole('button', { name: 'Đóng thông tin task' }));
    expect(container.querySelector('.task-detail-sidebar')).toBeNull();
    expect(container.querySelector('.task-detail-shell')).toHaveClass('sidebar-collapsed');
  });
  it('does not create a temporary turn bubble for ChatGPT queue events', async () => {
    chat();
    expect(screen.getByTestId('turn-children')).toHaveAttribute('data-turn-id', 'root-turn');
    act(() => scene.listener?.({ id: 'queue-created', type: 'chatgpt.queue.created', taskId: 'task-chat-root', occurredAt: '2026-09-06T00:00:01Z', payload: { messageId: 'queued-1' } }));
    await waitFor(() => expect(screen.getByTestId('turn-children')).toHaveAttribute('data-turn-id', 'root-turn'));
    expect(screen.getAllByTestId('turn-children')).toHaveLength(1);
  });
  it('refreshes the root when a grandchild status arrives on the immediate parent task', async () => {
    chat();
    act(() => scene.listener?.({ id: 'grandchild-update', type: 'subagent.status', taskId: 'task-child', occurredAt: '2026-09-06T00:00:00Z', payload: { childTaskId: 'task-grandchild', status: 'completed' } }));
    await waitFor(() => expect(scene.refresh).toHaveBeenCalled());
  });
  it('offers and persists zero from the execution settings tab', async () => {
    const settings = { bindAddress: '127.0.0.1', port: 8080, mcpEndpoint: '', databasePath: '', executionMode: 'approval', approveNewConversations: false, terminalExecutable: 'pwsh', taskConcurrency: 2, sessionConcurrency: 2, subagentConcurrency: 0, theme: 'dark', fontFamily: 'Inter', taskFontScale: 100, language: 'en', newAgentSound: false, finishedTaskSound: false, dataRetention: 'off' };
    scene.data = settings;
    const save = vi.spyOn(api, 'saveSettings').mockImplementation(async (value) => value);
    render(<MemoryRouter initialEntries={['/settings?tab=execution']}><SettingsPage /></MemoryRouter>);
    const select = await screen.findByRole('combobox', { name: /Sub-agent count/ });
    expect(select).toHaveValue('0');
    expect(within(select).getAllByRole('option').map((option) => option.getAttribute('value'))).toEqual(['0', '1', '2', '3', '4', '5']);
    fireEvent.change(select, { target: { value: '2' } });
    fireEvent.change(select, { target: { value: '0' } });
    fireEvent.submit(select.closest('form')!);
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ subagentConcurrency: 0 })));
  });
});
