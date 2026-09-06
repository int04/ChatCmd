import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError } from '../api';
import { resumeChatGptCompact } from '../chatgptBridge';
import { CompactSession } from '../chatgpt/compact/CompactSession';
import type { CompactJob } from '../chatgpt/compact/types';
import { compactJob, compactTaskId, newUrl } from './compactFixtures';

vi.mock('../chatgptBridge', () => ({ resumeChatGptCompact: vi.fn() }));
let stops: Array<() => void>;
beforeEach(() => {
  vi.useFakeTimers(); stops = [];
  vi.mocked(resumeChatGptCompact).mockReset().mockResolvedValue(undefined);
  vi.spyOn(api, 'chatGptCompact').mockResolvedValue({ active: null, history: [] });
  vi.spyOn(api, 'chatGptCompactJob').mockResolvedValue(compactJob());
  vi.spyOn(api, 'startChatGptCompact').mockResolvedValue(compactJob());
  vi.spyOn(api, 'cancelChatGptCompact').mockResolvedValue(compactJob({ phase: 'cancelled', revision: 2 }));
});
afterEach(() => { stops.forEach((stop) => stop()); vi.restoreAllMocks(); });
async function start() {
  const session = new CompactSession(compactTaskId);
  stops.push(session.start());
  await session.refresh();
  return session;
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('durable compact session', () => {
  it('only reads before confirmation and refreshes inactive history at 15 seconds', async () => {
    const session = await start();
    expect(session.isBlocked()).toBe(false);
    expect(api.startChatGptCompact).not.toHaveBeenCalled();
    expect(resumeChatGptCompact).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(14_999);
    expect(api.chatGptCompact).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(api.chatGptCompact).toHaveBeenCalledTimes(2);
  });

  it('wakes the persisted active job on mount and polls every 2s without launch flooding', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [compactJob()] });
    const session = await start();
    expect(resumeChatGptCompact).toHaveBeenCalledExactlyOnceWith('compact-test-job', compactTaskId);
    expect(session.getSnapshot().history).toHaveLength(1);
    vi.mocked(api.chatGptCompactJob).mockResolvedValue(compactJob({ phase: 'writing_handoff', revision: 2 }));
    await vi.advanceTimersByTimeAsync(6_000);
    expect(api.chatGptCompactJob).toHaveBeenCalledTimes(3);
    expect(session.getSnapshot().active?.phase).toBe('writing_handoff');
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(1);
    stops[0]();
    await vi.advanceTimersByTimeAsync(6_000);
    expect(api.chatGptCompactJob).toHaveBeenCalledTimes(3);
  });

  it('locks a double confirmation synchronously and wakes only the server-returned job', async () => {
    const session = await start();
    const pending = deferred<CompactJob>();
    vi.mocked(api.startChatGptCompact).mockReturnValue(pending.promise);
    const creating = session.create();
    await session.create();
    expect(session.isBlocked()).toBe(true);
    expect(api.startChatGptCompact).toHaveBeenCalledTimes(1);
    pending.resolve(compactJob()); await creating;
    expect(resumeChatGptCompact).toHaveBeenCalledExactlyOnceWith('compact-test-job', compactTaskId);
    await session.create();
    expect(api.startChatGptCompact).toHaveBeenCalledTimes(1);
  });

  it('recovers a successful POST whose response was lost, without posting another job', async () => {
    const session = await start();
    vi.mocked(api.startChatGptCompact).mockRejectedValue(new Error('response lost'));
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    await session.create(); await session.refresh();
    expect(session.getSnapshot().active?.id).toBe('compact-test-job');
    expect(api.startChatGptCompact).toHaveBeenCalledTimes(1);
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(1);
  });

  it('cancels by CAS revision and does not resurrect a cancelled job from a stale poll', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    const session = await start();
    const delayed = deferred<CompactJob>();
    vi.mocked(api.chatGptCompactJob).mockReturnValue(delayed.promise);
    const poll = session.refresh();
    await session.cancel();
    expect(api.cancelChatGptCompact).toHaveBeenCalledExactlyOnceWith('compact-test-job', 1);
    delayed.resolve(compactJob()); await poll;
    expect(session.getSnapshot().active).toBeNull();
    expect(session.getSnapshot().history[0].phase).toBe('cancelled');
    expect(session.isBlocked()).toBe(false);
  });

  it('refreshes a cancellation conflict but never automatically retries with a newer revision', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    const session = await start();
    vi.mocked(api.cancelChatGptCompact).mockRejectedValue(new ApiError('conflict', 409));
    vi.mocked(api.chatGptCompactJob).mockResolvedValue(compactJob({ revision: 3, phase: 'saving_handoff' }));
    await session.cancel();
    expect(api.cancelChatGptCompact).toHaveBeenCalledTimes(1);
    expect(session.getSnapshot().active?.revision).toBe(3);
    expect(session.getSnapshot().error).toMatch(/Progress changed|Tiến trình đã thay đổi/);
  });

  it('preserves active state after a polling failure and exposes a retryable error', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    const session = await start();
    vi.mocked(api.chatGptCompactJob).mockRejectedValueOnce(new Error('offline'));
    await vi.advanceTimersByTimeAsync(2_000);
    expect(session.isBlocked()).toBe(true);
    expect(session.getSnapshot().error).toContain('offline');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(session.getSnapshot().error).toBe('');
  });

  it('refreshes and deduplicates history on completion, retaining the same task', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValueOnce({ active: compactJob(), history: [] });
    const session = await start();
    const done = compactJob({ phase: 'completed', revision: 5, newConversationId: 'new-chat', newConversationUrl: newUrl });
    vi.mocked(api.chatGptCompactJob).mockResolvedValue(done);
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: null, history: [done, done, compactJob({ id: 'older', phase: 'cancelled', createdAtMs: 1 })] });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(session.getSnapshot().history.map((job) => job.id)).toEqual(['compact-test-job', 'older']);
    expect(session.getSnapshot().active).toBeNull();
    expect(api.chatGptCompact).toHaveBeenLastCalledWith(compactTaskId);
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(1);
  });

  it('fails closed when both a POST response and the recovery read are unavailable', async () => {
    const session = await start();
    vi.mocked(api.startChatGptCompact).mockRejectedValue(new Error('response lost'));
    vi.mocked(api.chatGptCompact).mockRejectedValue(new Error('offline'));
    await session.create(); await session.refresh();
    expect(session.isBlocked()).toBe(true);
    expect(session.getSnapshot().ready).toBe(false);
    await session.create();
    expect(api.startChatGptCompact).toHaveBeenCalledTimes(1);
  });

  it('rejects cross-task results and cannot enable sends on an unknown initial state', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob({ taskId: 'other-task' }), history: [] });
    const session = await start();
    expect(session.isBlocked()).toBe(true);
    expect(session.getSnapshot().error).toContain('task mismatch');
    expect(resumeChatGptCompact).not.toHaveBeenCalled();
  });
});
