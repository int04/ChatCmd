import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { api } from '../api';
import { setAppLanguage } from '../i18n';
import { TaskTurnBubble } from '../tasks/TaskTurnBubble';
import type { SubagentRun, TaskDetail, TaskTurn, TimelineEvent } from '../types';

const realtime = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (listener: (event: TimelineEvent) => void) => { realtime.listener = listener; return 'online'; } }));

const startedAt = '2026-09-10T00:00:00Z';

function childAgent(): SubagentRun {
  return {
    id: 'subagent-child',
    parentTurnId: 'root-turn',
    parentTaskId: 'task-chat-root',
    rootTurnId: 'root-turn',
    taskId: 'task-chat-child',
    name: 'Child reviewer',
    request: 'Review the child task',
    status: 'completed',
    createdAtUtc: startedAt,
    updatedAtUtc: startedAt,
    completedAtUtc: startedAt,
    attempt: 1,
    maxRuntimeMs: 1_800_000,
  };
}

function parentTurn(): TaskTurn {
  return { id: 'root-turn', status: 'running', startedAtUtc: startedAt, events: [] };
}

function childDetail(): TaskDetail {
  return {
    task: {
      id: 'task-chat-child',
      source: 'chatgpt_web',
      title: 'Child conversation',
      status: 'running',
      updatedAtUtc: startedAt,
      isSubagent: true,
    },
    turns: [{
      id: 'child-turn',
      status: 'running',
      startedAtUtc: startedAt,
      events: [{
        id: 'child-user-message',
        type: 'message',
        taskId: 'task-chat-child',
        turnId: 'child-turn',
        occurredAt: startedAt,
        payload: { role: 'user', content: 'Child body content' },
      }],
    }],
    subagents: [],
  };
}

beforeEach(() => {
  setAppLanguage('en', false);
  realtime.listener = null;
});

afterEach(() => {
  vi.restoreAllMocks();
});

it('opens a body-only subagent preview and keeps the full conversation in a new-tab action', async () => {
  const taskSpy = vi.spyOn(api, 'task').mockResolvedValue(childDetail());
  const { container } = render(<TaskTurnBubble turn={parentTurn()} taskId="task-chat-root" subagents={[childAgent()]} />);

  const trigger = screen.getByRole('button', { name: /Child reviewer - Done - Preview conversation/ });
  expect(trigger).toHaveAttribute('aria-haspopup', 'dialog');
  fireEvent.click(trigger);

  await waitFor(() => expect(taskSpy).toHaveBeenCalledWith('task-chat-child'));
  const dialog = await screen.findByRole('dialog');
  expect(within(dialog).getByText('Child body content')).toBeInTheDocument();
  expect(dialog.querySelector('.task-detail-topbar')).toBeNull();
  expect(dialog.querySelector('.task-detail-sidebar')).toBeNull();
  expect(dialog.querySelector('.task-chat-footer')).toBeNull();
  expect(dialog.querySelector('.subagent-preview-timeline')).not.toBeNull();

  act(() => realtime.listener?.({
    id: 'child-progress',
    type: 'progress',
    taskId: 'task-chat-child',
    turnId: 'child-turn',
    occurredAt: '2026-09-10T00:00:01Z',
    payload: { message: 'Realtime child update' },
  }));
  await waitFor(() => expect(within(dialog).getByText('Realtime child update')).toBeInTheDocument());

  act(() => realtime.listener?.({
    id: 'grandchild-status',
    type: 'subagent.status',
    taskId: 'task-chat-child',
    turnId: 'child-turn',
    occurredAt: '2026-09-10T00:00:02Z',
    payload: { childTaskId: 'task-grandchild', status: 'running' },
  }));
  await waitFor(() => expect(taskSpy).toHaveBeenCalledTimes(2));
  expect(within(dialog).getByText('Realtime child update')).toBeInTheDocument();

  const openConversation = within(dialog).getByRole('link', { name: /Go to conversation/ });
  expect(openConversation).toHaveAttribute('href', '/tasks/task-chat-child');
  expect(openConversation).toHaveAttribute('target', '_blank');
  expect(openConversation).toHaveAttribute('rel', expect.stringContaining('noopener'));
  expect(container.querySelector('a.turn-subagent')).toBeNull();
});
