import { describe, expect, it } from 'vitest';

import { mergeTasks } from '../tasks/TaskRail';
import type { Task } from '../types';

function task(status: string, updatedAtUtc: string): Task {
  return { id: 'task-1', status, updatedAtUtc };
}

describe('task rail refresh merging', () => {
  it('does not let an older running response overwrite a newer realtime completion', () => {
    const staleServer = task('running', '2026-08-27T08:00:04.000Z');
    const realtimeCompleted = task('completed', '2026-08-27T08:00:05.000Z');

    expect(mergeTasks([staleServer], [realtimeCompleted])[0].status).toBe('completed');
  });

  it('accepts a genuinely newer server state', () => {
    const current = task('completed', '2026-08-27T08:00:05.000Z');
    const newerServer = task('running', '2026-08-27T08:01:00.000Z');

    expect(mergeTasks([newerServer], [current])[0].status).toBe('running');
  });
});
