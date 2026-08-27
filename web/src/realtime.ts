import { useEffect, useState } from 'react';
import type { RealtimeState, TimelineEvent } from './types';

export function useRealtime(onEvent: (event: TimelineEvent) => void, WebSocketImpl: typeof WebSocket = WebSocket) {
  const [state, setState] = useState<RealtimeState>('offline');
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
          seen.add(event.id); if (seen.size > 2000) seen.delete(seen.values().next().value!);
          onEvent(event);
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
    return () => { stopped = true; if (timer) clearTimeout(timer); socket?.close(); };
  }, [WebSocketImpl, onEvent]);
  return state;
}
