import { act, render, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { TimelineEvent } from '../types';
import { GlobalSubagentFallbackBridge } from '../tasks/GlobalSubagentFallbackBridge';
import { closeSubagentFallbackTab } from '../chatgptBridge';

const events = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (callback: (event: TimelineEvent) => void) => { events.listener = callback; return 'online'; } }));
vi.mock('../api', () => ({ api: { pendingSubagentFallbacks: vi.fn(async () => []) } }));
vi.mock('../chatgptBridge', () => ({ closeSubagentFallbackTab: vi.fn(async () => undefined), dispatchSubagentFallback: vi.fn() }));

it('keeps a claimed/completed MCP child tab alive until its browser final answer, but closes failed work', async () => {
  render(<GlobalSubagentFallbackBridge />);
  const event = (type: string, status?: string): TimelineEvent => ({ id: type, type, occurredAt: '2026-09-06T00:00:00Z', payload: { subagentId: 'child', status } });
  act(() => events.listener?.(event('subagent.fallback_claimed')));
  act(() => events.listener?.(event('subagent.status', 'completed')));
  expect(closeSubagentFallbackTab).not.toHaveBeenCalled();
  act(() => events.listener?.(event('subagent.status', 'timedOut')));
  await waitFor(() => expect(closeSubagentFallbackTab).toHaveBeenCalledWith('child'));
});
