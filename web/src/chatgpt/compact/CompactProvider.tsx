import { createContext, useContext, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from 'react';
import { useRealtime } from '../../realtime';
import { CompactSession, type CompactSnapshot } from './CompactSession';
import type { CompactJob } from './types';
import './compact.css';

export interface CompactController extends CompactSnapshot {
  blocked: boolean;
  latestCompleted: CompactJob | undefined;
  create: (continueAfterCompact?: boolean) => Promise<void>;
  cancel: () => Promise<void>;
  resume: () => Promise<void>;
  refresh: (recover?: boolean) => Promise<void>;
  isBlocked: () => boolean;
}

const CompactContext = createContext<CompactController | null>(null);
export const useCompact = () => useContext(CompactContext);

export function CompactProvider({ taskId, enabled = true, children }: { taskId: string; enabled?: boolean; children: ReactNode }) {
  return enabled ? <TaskCompactProvider key={taskId} taskId={taskId}>{children}</TaskCompactProvider> : children;
}

function TaskCompactProvider({ taskId, children }: { taskId: string; children: ReactNode }) {
  const [session] = useState(() => new CompactSession(taskId));
  const snapshot = useSyncExternalStore(session.subscribe, session.getSnapshot);
  useEffect(() => session.start(), [session]);
  useRealtime((event) => {
    if (event.type === 'system.connected' || event.type === 'system.resync_required') void session.refresh(true);
    else if (event.type === 'chatgpt_compact_updated' && (!event.taskId || event.taskId === taskId)) void session.refresh();
  });
  useEffect(() => {
    const recover = () => { void session.refresh(true); };
    const visible = () => { if (document.visibilityState === 'visible') recover(); };
    window.addEventListener('online', recover);
    window.addEventListener('focus', recover);
    document.addEventListener('visibilitychange', visible);
    return () => {
      window.removeEventListener('online', recover);
      window.removeEventListener('focus', recover);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [session]);
  const value = useMemo<CompactController>(() => ({
    ...snapshot,
    blocked: !snapshot.ready || snapshot.busy || Boolean(snapshot.active),
    latestCompleted: snapshot.history.find((job) => job.phase === 'completed'),
    create: session.create, cancel: session.cancel, resume: session.resume,
    refresh: session.refresh, isBlocked: session.isBlocked,
  }), [snapshot, session]);
  return <CompactContext.Provider value={value}>{children}</CompactContext.Provider>;
}
