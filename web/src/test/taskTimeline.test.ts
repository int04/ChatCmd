import { describe, expect, it } from 'vitest';
import type { TimelineEvent } from '../types';
import { activityCodeView, activityOutput, buildProcessBlocks, findUserMessage, type ToolActivity, upsertTaskEvent } from '../tasks/taskTimeline';

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

describe('git activity editor output', () => {
  function gitActivity(tool: string, stdout: string, stderr = ''): ToolActivity {
    return {
      id: `activity-${tool}`,
      tool,
      kind: 'git',
      input: { cwd: 'D:\\DEV\\CmdGPT\\ChatCmdClient' },
      output: { exitCode: 0, stdout, stderr, truncated: false },
      status: 'succeeded',
      startedAt: '2026-08-27T08:00:00.000Z',
      finishedAt: '2026-08-27T08:00:01.000Z',
    };
  }

  it('extracts real stdout and stderr from git CommandOutput', () => {
    const activity = gitActivity('git_status', '## main\n M web/src/tasks/taskTimeline.ts', 'warning text');
    expect(activityOutput(activity)).toBe('## main\n M web/src/tasks/taskTimeline.ts\nwarning text');
  });

  it('renders every git tool in the code viewer and highlights diffs', () => {
    const status = gitActivity('git_status', '## main\n M file.rs');
    const diff = gitActivity('git_diff', 'diff --git a/file.rs b/file.rs\n+new line');
    expect(activityCodeView(status)).toMatchObject({ code: '## main\n M file.rs', language: 'plain' });
    expect(activityCodeView(diff)).toMatchObject({ code: 'diff --git a/file.rs b/file.rs\n+new line', language: 'diff' });
  });
});
describe('user message synchronization rendering', () => {
  it('extracts the user message and keeps it out of agent progress blocks', () => {
    const user: TimelineEvent = {
      id: 'user-message-1', type: 'message', occurredAt: '2026-08-27T08:00:00.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { role: 'user', content: 'Hãy kiểm tra repo này' },
    };
    const progress: TimelineEvent = {
      id: 'progress-1', type: 'progress', occurredAt: '2026-08-27T08:00:01.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { content: 'Đang kiểm tra.' },
    };
    expect(findUserMessage([user, progress])?.text).toBe('Hãy kiểm tra repo này');
    const blocks = buildProcessBlocks([user, progress]);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ type: 'progress' });
  });
});