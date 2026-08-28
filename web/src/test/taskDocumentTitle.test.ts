import { describe, expect, it } from 'vitest';

import { titleLabelForEvent } from '../tasks/taskDocumentTitle';
import type { TimelineEvent } from '../types';

function event(type: string, payload: unknown): TimelineEvent {
  return { id: `${type}-1`, type, taskId: 'task-1', occurredAt: '2026-08-28T10:00:00.000Z', payload };
}

describe('task document title labels', () => {
  it('describes file, search, git, and command activity like the legacy client', () => {
    expect(titleLabelForEvent(event('tool_call', { tool: 'fs_read_text', input: { path: 'D:\\DEV\\CmdGPT\\ChatCmdClient\\src\\main.rs' } })))
      .toBe('Đang đọc D:\\DEV\\CmdGPT\\ChatCmdClient\\src\\main.rs');
    expect(titleLabelForEvent(event('tool_call', { tool: 'fs_search', input: { query: 'document.title' } })))
      .toBe('Đang tìm document.title');
    expect(titleLabelForEvent(event('tool_call', { tool: 'git_status', input: { workingDirectory: 'D:\\DEV\\CmdGPT\\ChatCmdClient' } })))
      .toBe('Đang thao tác Git · D:\\DEV\\CmdGPT\\ChatCmdClient');
    expect(titleLabelForEvent(event('tool_call', { tool: 'shell_write', input: { text: 'npm run build' } })))
      .toBe('Đang chạy lệnh');
  });

  it('returns to thinking after tools and clears when the task completes', () => {
    expect(titleLabelForEvent(event('tool_result', { tool: 'fs_read_text', status: 'succeeded' }))).toBe('Đang suy nghĩ');
    expect(titleLabelForEvent(event('progress', { message: 'Checking files' }))).toBe('Đang suy nghĩ');
    expect(titleLabelForEvent(event('status', { status: 'completed' }))).toBeNull();
  });
});
