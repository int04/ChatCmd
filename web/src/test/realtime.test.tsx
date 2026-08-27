import { act, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RealtimeProvider, useRealtime } from '../realtime';
import { FakeSocket } from './setup';

const wrapper = ({ children }: { children: ReactNode }) => <RealtimeProvider WebSocketImpl={FakeSocket as unknown as typeof WebSocket}>{children}</RealtimeProvider>;

describe('realtime reconnect', () => {
  beforeEach(() => { FakeSocket.instances.length = 0; });
  it('reports reconnecting, reconnects exponentially, and deduplicates IDs', () => {
    vi.useFakeTimers();
    const receive = vi.fn();
    const { result, unmount } = renderHook(() => useRealtime(receive), { wrapper });
    expect(result.current).toBe('offline');
    act(() => FakeSocket.instances[0].open());
    expect(result.current).toBe('online');
    act(() => {
      FakeSocket.instances[0].message({ id: 'evt-1', type: 'task.updated', occurredAt: '2026-01-01' });
      FakeSocket.instances[0].message({ id: 'evt-1', type: 'task.updated', occurredAt: '2026-01-01' });
    });
    expect(receive).toHaveBeenCalledTimes(1);
    act(() => FakeSocket.instances[0].disconnect());
    expect(result.current).toBe('reconnecting');
    act(() => vi.advanceTimersByTime(500));
    expect(FakeSocket.instances).toHaveLength(2);
    unmount();
    act(() => vi.runOnlyPendingTimers());
    expect(FakeSocket.instances).toHaveLength(2);
  });

  it('keeps one socket when the subscriber callback changes', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { rerender } = renderHook(({ listener }) => useRealtime(listener), { initialProps: { listener: first }, wrapper });
    expect(FakeSocket.instances).toHaveLength(1);
    act(() => FakeSocket.instances[0].open());
    rerender({ listener: second });
    expect(FakeSocket.instances).toHaveLength(1);
    act(() => FakeSocket.instances[0].message({ id: 'evt-callback', type: 'task.updated', occurredAt: '2026-01-01' }));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });
});
