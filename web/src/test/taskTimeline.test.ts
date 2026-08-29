import { describe, expect, it } from 'vitest';
import type { TimelineEvent } from '../types';
import { activityCodeView, activityOutput, buildProcessBlocks, findUserMessage, mergeLiveDetail, type ToolActivity, upsertTaskEvent } from '../tasks/taskTimeline';

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
    expect(tasks?.[0]).toMatchObject({ status: 'completed', outputPreview: 'Đã hoàn tất.', finalResponseCount: 1 });
  });

  it('propagates stopped status from realtime without creating another final response', () => {
    const running: TimelineEvent = {
      id: 'event-running', type: 'progress', occurredAt: '2026-08-27T08:00:00.000Z', taskId: 'task-stop', payload: { status: 'running', content: 'Working' },
    };
    const stopped: TimelineEvent = {
      id: 'event-stopped', type: 'status', occurredAt: '2026-08-27T08:00:02.000Z', taskId: 'task-stop', payload: { status: 'stopped', content: 'Conversation stopped' },
    };
    const tasks = upsertTaskEvent(upsertTaskEvent([], running), stopped);
    expect(tasks?.[0]).toMatchObject({ status: 'stopped', finalResponseCount: 0 });
  });
});

describe('task final-response status reconciliation', () => {
  it('treats a task as completed when persisted status is stale but the latest turn has a final response', () => {
    const completed: TimelineEvent = {
      id: 'final-1', type: 'status', occurredAt: '2026-08-27T08:00:05.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { status: 'completed', content: 'Xong rồi.' },
    };
    const finalizerResult: TimelineEvent = {
      id: 'finalizer-result', type: 'tool_result', occurredAt: '2026-08-27T08:00:06.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { tool: 'agent_turn_complete', status: 'succeeded' },
    };
    const detail = mergeLiveDetail({ task: { id: 'task-1', status: 'running', updatedAtUtc: '2026-08-27T08:00:06.000Z' }, events: [completed, finalizerResult] }, []);
    expect(detail.task.status).toBe('completed');
  });

  it('keeps running when a genuinely newer turn starts after the previous final response', () => {
    const completed: TimelineEvent = {
      id: 'final-1', type: 'status', occurredAt: '2026-08-27T08:00:05.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { status: 'completed', content: 'Xong lượt trước.' },
    };
    const nextTurn: TimelineEvent = {
      id: 'next-turn', type: 'message', occurredAt: '2026-08-27T08:01:00.000Z',
      taskId: 'task-1', turnId: 'turn-2', payload: { role: 'user', content: 'Làm tiếp.' },
    };
    const detail = mergeLiveDetail({ task: { id: 'task-1', status: 'running', updatedAtUtc: '2026-08-27T08:01:00.000Z' }, events: [completed, nextTurn] }, []);
    expect(detail.task.status).toBe('running');
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

describe('sub-agent orchestration rendering', () => {
  it('keeps orchestration tools out of the visible process activity list', () => {
    const events: TimelineEvent[] = [
      {
        id: 'subagent-start', type: 'tool_call', occurredAt: '2026-08-27T08:00:00.000Z',
        taskId: 'task-parent', turnId: 'turn-parent', payload: { tool: 'agent_subagent_start', status: 'started' },
      },
      {
        id: 'subagent-wait', type: 'tool_call', occurredAt: '2026-08-27T08:00:01.000Z',
        taskId: 'task-parent', turnId: 'turn-parent', payload: { tool: 'agent_subagent_wait', status: 'started' },
      },
    ];
    expect(buildProcessBlocks(events)).toEqual([]);
  });
});
describe('running tool stop projection', () => {
  const started: TimelineEvent = {
    id: 'tool-start', type: 'tool_call', occurredAt: '2026-08-27T08:00:00.000Z',
    taskId: 'task-1', turnId: 'turn-1', payload: { activityId: 'activity-1', tool: 'git_diff', status: 'started', input: { cwd: '.' } },
  };

  it('projects a stop request onto the existing running activity', () => {
    const stopRequested: TimelineEvent = {
      id: 'tool-stop-request', type: 'tool_call', occurredAt: '2026-08-27T08:00:01.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { activityId: 'activity-1', tool: 'git_diff', status: 'stop_requested', stopReason: 'Đổi cách làm' },
    };
    const blocks = buildProcessBlocks([started, stopRequested]);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ type: 'activities', activities: [{ id: 'activity-1', status: 'stop_requested', error: 'Stop reason: Đổi cách làm' }] });
  });

  it('finishes the same activity as stopped and preserves the agent-facing reason', () => {
    const stopped: TimelineEvent = {
      id: 'tool-stopped', type: 'tool_result', occurredAt: '2026-08-27T08:00:02.000Z',
      taskId: 'task-1', turnId: 'turn-1', payload: { activityId: 'activity-1', tool: 'git_diff', status: 'stopped', errorCode: 'activity_stopped', errorMessage: 'the user stopped this activity. Reason: Đổi cách làm' },
    };
    const blocks = buildProcessBlocks([started, stopped]);
    expect(blocks[0]).toMatchObject({ type: 'activities', activities: [{ id: 'activity-1', status: 'stopped', error: 'the user stopped this activity. Reason: Đổi cách làm' }] });
  });
});
