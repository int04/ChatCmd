import { describe, expect, it } from 'vitest';
import type { Task, TimelineEvent } from '../types';
import { mergeLiveDetail, mergeTaskEvent } from './taskEventMerge';
import { buildTaskTurns } from './taskTimeline';

const task: Task = { id: 'task-native', source: 'chatgpt_web', status: 'completed', updatedAtUtc: '2026-09-05T10:00:00Z' };
const question: TimelineEvent = { id: 'user-2', taskId: task.id, turnId: 'browser-turn-2', type: 'message', occurredAt: '2026-09-05T10:01:00Z', payload: { role: 'user', provider: 'chatgpt_web', content: 'A direct follow-up' } };
const snapshot: TimelineEvent = { id: 'think-2', taskId: task.id, turnId: question.turnId, type: 'chatgpt_think', occurredAt: '2026-09-05T10:01:01Z', payload: { revision: 1, messages: [{ id: 'part', kind: 'commentary', content: 'Working on it' }], completed: false } };

describe('native browser capture lifecycle', () => {
  it('shows a follow-up as running before any MCP signal arrives', () => {
    const detail = mergeLiveDetail({ task, events: [] }, [question, snapshot]);
    expect(detail.task.status).toBe('running');
    const turns = buildTaskTurns(detail.events ?? [], detail.task);
    expect(turns).toHaveLength(1);
    expect(turns[0].status).toBe('running');
    expect(turns[0].events).toContainEqual(snapshot);
  });

  it('does not reopen a completed task when an older question is replayed', () => {
    expect(mergeTaskEvent(task, { ...question, occurredAt: '2026-09-05T09:00:00Z' }).status).toBe('completed');
  });

  it('keeps content-only snapshots independent of task lifecycle', () => {
    expect(mergeTaskEvent(task, snapshot).status).toBe('completed');
    expect(mergeTaskEvent({ ...task, status: 'stopped' }, question).status).toBe('stopped');
  });
});
