import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useRealtime } from '../realtime';
import { FakeSocket } from './setup';

describe('realtime reconnect', () => {
  it('reports reconnecting, reconnects exponentially, and deduplicates IDs', () => { vi.useFakeTimers(); const receive = vi.fn(); const { result, unmount } = renderHook(() => useRealtime(receive, FakeSocket as unknown as typeof WebSocket)); expect(result.current).toBe('offline'); act(() => FakeSocket.instances[0].open()); expect(result.current).toBe('online'); act(() => { FakeSocket.instances[0].message({ id: 'evt-1', type: 'task.updated', occurredAt: '2026-01-01' }); FakeSocket.instances[0].message({ id: 'evt-1', type: 'task.updated', occurredAt: '2026-01-01' }); }); expect(receive).toHaveBeenCalledTimes(1); act(() => FakeSocket.instances[0].disconnect()); expect(result.current).toBe('reconnecting'); act(() => vi.advanceTimersByTime(500)); expect(FakeSocket.instances).toHaveLength(2); unmount(); act(() => vi.runOnlyPendingTimers()); expect(FakeSocket.instances).toHaveLength(2); });
});
