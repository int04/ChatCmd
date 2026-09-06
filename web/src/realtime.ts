import { realtimeEventKey } from './tasks/timelineSnapshots';
import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { RealtimeState, TimelineEvent } from './types';

type RealtimeListener = (event: TimelineEvent) => void;
type RealtimeContextValue = { state: RealtimeState; subscribe: (listener: RealtimeListener) => () => void };

const RealtimeContext = createContext<RealtimeContextValue | null>(null);

export function webSocketAuthority(endpoint: Pick<Location, 'protocol' | 'hostname' | 'host' | 'port'> = location): string {
  // The local Rust listener binds IPv4 by default; localhost may resolve to ::1.
  // Preserve the selected port (including Vite's proxy) and HTTPS certificate host.
  if (endpoint.protocol !== 'http:' || endpoint.hostname !== 'localhost') return endpoint.host;
  return endpoint.port ? `127.0.0.1:${endpoint.port}` : '127.0.0.1';
}

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
      const currentSocket = new WebSocketImpl(`${protocol}//${webSocketAuthority()}/ws`);
      socket = currentSocket;

      currentSocket.onopen = () => {
        attempt = 0;
        setState('online');
        currentSocket.send(JSON.stringify({ type: 'client.ready' }));
      };

      currentSocket.onmessage = ({ data }) => {
        try {
          const event = parseRealtimeEvent(data);
          if (!event.id || !event.type || seen.has(realtimeEventKey(event))) return;
          seen.add(realtimeEventKey(event));
          if (seen.size > 2000) seen.delete(seen.values().next().value!);
          for (const listener of listeners) listener(event);
        } catch {
          socket?.close();
        }
      };

      currentSocket.onerror = () => socket?.close();
      currentSocket.onclose = () => {
        if (stopped) return;
        setState('reconnecting');
        const delay = Math.min(30_000, 500 * 2 ** attempt++);
        timer = window.setTimeout(() => void connect(), delay);
      };
    };

    void connect();
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

function parseRealtimeEvent(data: string | ArrayBuffer | Blob): TimelineEvent {
  if (typeof data !== 'string') throw new Error('WebSocket event must be plaintext JSON');
  return JSON.parse(data) as TimelineEvent;
}

export function useRealtime(onEvent: RealtimeListener) {
  const realtime = useContext(RealtimeContext);
  const listenerRef = useRef(onEvent);
  useEffect(() => {
    listenerRef.current = onEvent;
  }, [onEvent]);
  useEffect(() => realtime?.subscribe((event) => listenerRef.current(event)), [realtime]);
  if (!realtime) throw new Error('useRealtime must be used within RealtimeProvider');
  return realtime.state;
}
