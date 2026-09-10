import { act, render, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { TimelineEvent } from '../types';
import { api } from '../api';
import { GlobalSubagentFallbackBridge } from '../tasks/GlobalSubagentFallbackBridge';
import { closeSubagentFallbackTab, dispatchSubagentFallback } from '../chatgptBridge';

const events = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (callback: (event: TimelineEvent) => void) => { events.listener = callback; return 'online'; } }));
vi.mock('../api', () => ({ api: { pendingSubagentFallbacks: vi.fn(async () => []) } }));
vi.mock('../chatgptBridge', () => ({ closeSubagentFallbackTab: vi.fn(async () => undefined), dispatchSubagentFallback: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValue([]);
});

it('does not auto-open a brand-new browser child while recovering stale pending fallback', async () => {
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValueOnce([{
    subagentId: 'stale-child',
    childTaskId: 'task-stale-child',
    name: 'Stale child',
    submittedContent: 'Read one file',
    attempt: 1,
    maxAttempts: 3,
    conversationUrl: null,
  }]);
  render(<GlobalSubagentFallbackBridge />);
  await waitFor(() => expect(api.pendingSubagentFallbacks).toHaveBeenCalled());
  expect(dispatchSubagentFallback).not.toHaveBeenCalled();
});

it('resumes recovery when the browser child already has a conversation URL', async () => {
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValueOnce([{
    subagentId: 'existing-child',
    childTaskId: 'task-existing-child',
    name: 'Existing child',
    submittedContent: 'Read one file',
    attempt: 1,
    maxAttempts: 3,
    conversationUrl: 'https://chatgpt.com/c/existing-child',
  }]);
  render(<GlobalSubagentFallbackBridge />);
  await waitFor(() => expect(dispatchSubagentFallback).toHaveBeenCalledWith(expect.objectContaining({
    subagentId: 'existing-child',
    conversationUrl: 'https://chatgpt.com/c/existing-child',
  })));
});

it('keeps a claimed/completed MCP child tab alive until its browser final answer, but closes failed work', async () => {
  render(<GlobalSubagentFallbackBridge />);
  const event = (type: string, status?: string): TimelineEvent => ({ id: type, type, occurredAt: '2026-09-06T00:00:00Z', payload: { subagentId: 'child', status } });
  act(() => events.listener?.(event('subagent.fallback_claimed')));
  act(() => events.listener?.(event('subagent.status', 'completed')));
  expect(closeSubagentFallbackTab).not.toHaveBeenCalled();
  act(() => events.listener?.(event('subagent.status', 'timedOut')));
  await waitFor(() => expect(closeSubagentFallbackTab).toHaveBeenCalledWith('child'));
});
