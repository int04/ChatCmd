import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import type { RealtimeState, TimelineEvent } from './types';

type RealtimeListener = (event: TimelineEvent) => void;
type RealtimeContextValue = { state: RealtimeState; subscribe: (listener: RealtimeListener) => () => void };

const RealtimeContext = createContext<RealtimeContextValue | null>(null);

export function RealtimeProvider({ children, WebSocketImpl = WebSocket }: { children: ReactNode; WebSocketImpl?: typeof WebSocket }) {
  const [state, setState] = useState<RealtimeState>('offline');
  const [listeners] = useState(() => new Set<RealtimeListener>());

  useEffect(() => {
    let socket: WebSocket | undefined;
    let timer: number | undefined;
    let stopped = false;
    let attempt = 0;
    const seen = new Set<string>();

    const connect = () => {
      if (stopped) return;
      setState(attempt ? 'reconnecting' : 'offline');
      const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      socket = new WebSocketImpl(`${protocol}//${location.host}/ws`);
      socket.onopen = () => { attempt = 0; setState('online'); };
      socket.onmessage = ({ data }) => {
        try {
          const event = JSON.parse(String(data)) as TimelineEvent;
          if (!event.id || !event.type || seen.has(event.id)) return;
          seen.add(event.id);
          if (seen.size > 2000) seen.delete(seen.values().next().value!);
          for (const listener of listeners) listener(event);
        } catch { /* malformed events are ignored */ }
      };
      socket.onerror = () => socket?.close();
      socket.onclose = () => {
        if (stopped) return;
        setState('reconnecting');
        const delay = Math.min(30_000, 500 * 2 ** attempt++);
        timer = window.setTimeout(connect, delay);
      };
    };

    connect();
    return () => {
      stopped = true;
      if (timer) window.clearTimeout(timer);
      socket?.close();
    };
  }, [WebSocketImpl, listeners]);

  const subscribe = useCallback((listener: RealtimeListener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }, [listeners]);
  const value = useMemo<RealtimeContextValue>(() => ({ state, subscribe }), [state, subscribe]);

  return createElement(RealtimeContext.Provider, { value }, children);
}

export function useRealtime(onEvent: RealtimeListener) {
  const realtime = useContext(RealtimeContext);
  useEffect(() => realtime?.subscribe(onEvent), [realtime, onEvent]);
  if (!realtime) throw new Error('useRealtime must be used within RealtimeProvider');
  return realtime.state;
}
