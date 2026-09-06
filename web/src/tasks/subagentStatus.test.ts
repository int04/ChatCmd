import { describe, expect, it } from 'vitest';
import { subagentStatusText } from './TaskTurnBubble';

describe('subagent status details', () => {
  it('shows retry attempt and terminal timeout reason', () => {
    expect(subagentStatusText({ attempt: 2, terminalReason: 'child heartbeat lease expired' }, 'Timed out'))
      .toBe('Timed out · Attempt 2 · child heartbeat lease expired');
  });
});
