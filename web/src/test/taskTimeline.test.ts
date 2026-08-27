import { describe, expect, it } from 'vitest';
import type { TimelineEvent } from '../types';
import { upsertTaskEvent } from '../tasks/taskTimeline';

describe('task realtime list updates', () => {
  it('adds a brand new conversation when its first websocket event arrives', () => {
    const event: TimelineEvent = {
      id: 'event-1',
      type: 'tool_call',
      occurredAt: '2026-08-27T08:00:00.000Z',
      taskId: 'task-new-chat',
      turnId: 'turn-1',
      sessionId: 'session-1',
      payload: { tool: 'fs_read_text', status: 'started' },
    };

    const tasks = upsertTaskEvent([], event);

    expect(tasks).toHaveLength(1);
    expect(tasks?.[0]).toMatchObject({
      id: 'task-new-chat',
      status: 'running',
      activeSessionId: 'session-1',
      turnCount: 1,
    });
  });

  it('updates an existing conversation instead of inserting a duplicate', () => {
    const first: TimelineEvent = {
      id: 'event-1',
      type: 'tool_call',
      occurredAt: '2026-08-27T08:00:00.000Z',
      taskId: 'task-new-chat',
      turnId: 'turn-1',
      payload: { tool: 'fs_read_text', status: 'started' },
    };
    const completed: TimelineEvent = {
      id: 'event-2',
      type: 'status',
      occurredAt: '2026-08-27T08:00:05.000Z',
      taskId: 'task-new-chat',
      turnId: 'turn-1',
      payload: { status: 'completed', content: 'Đã hoàn tất.' },
    };

    const tasks = upsertTaskEvent(upsertTaskEvent([], first), completed);

    expect(tasks).toHaveLength(1);
    expect(tasks?.[0]).toMatchObject({ status: 'completed', outputPreview: 'Đã hoàn tất.' });
  });
});
