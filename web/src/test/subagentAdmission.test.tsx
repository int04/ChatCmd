import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { api } from '../api';
import { dispatchSubagentFallback } from '../chatgptBridge';
import { ChatGptBridgeTimeoutError } from '../chatgpt/bridgeErrors';
import { GlobalSubagentFallbackBridge } from '../tasks/GlobalSubagentFallbackBridge';
import type { TimelineEvent } from '../types';

const events = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (listener: (event: TimelineEvent) => void) => {
  events.listener = listener; return 'online';
} }));
vi.mock('../api', () => ({ api: {
  pendingSubagentFallbacks: vi.fn(async () => []), reportSubagentFallbackResult: vi.fn(async () => ({})),
} }));
vi.mock('../chatgptBridge', () => ({ dispatchSubagentFallback: vi.fn(), closeSubagentFallbackTab: vi.fn() }));
beforeEach(() => vi.clearAllMocks());
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const event = (id = 'child'): TimelineEvent => ({ id: `event-${id}`, type: 'subagent.fallback_requested',
  occurredAt: '2026-09-13T00:00:00Z', payload: { subagentId: id, childTaskId: `task-${id}`,
    submittedContent: 'Read one file', attempt: 1 } });

it('lost admission ACK must not fail or advance an already dispatched child', async () => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.mocked(dispatchSubagentFallback).mockRejectedValueOnce(new ChatGptBridgeTimeoutError('no ACK'));
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event()); });
  await waitFor(() => expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1));
  expect(api.reportSubagentFallbackResult).not.toHaveBeenCalled();
});

it('an explicit dispatch rejection still reports the failed attempt once', async () => {
  vi.mocked(dispatchSubagentFallback).mockRejectedValueOnce(new Error('invalid dispatch'));
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event()); });
  await waitFor(() => expect(api.reportSubagentFallbackResult).toHaveBeenCalledWith('child',
    expect.objectContaining({ attempt: 1, status: 'failed', errorMessage: 'invalid dispatch' })));
});

it('concurrent children have independent admission state and duplicate events are coalesced', async () => {
  let resolve!: () => void;
  const pending = new Promise<void>((done) => { resolve = done; });
  vi.mocked(dispatchSubagentFallback).mockReturnValue(pending);
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event('one')); events.listener?.(event('one')); events.listener?.(event('two')); });
  expect(dispatchSubagentFallback).toHaveBeenCalledTimes(2);
  await act(async () => { resolve(); await pending; });
  expect(api.reportSubagentFallbackResult).not.toHaveBeenCalled();
});
