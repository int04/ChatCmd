import { useEffect } from 'react';
import type { ChatGptBridge } from '../../types';
import { useCompact } from './CompactProvider';

/** Refresh in place: never remount the composer/queue or navigate to another task. */
export function useCompactBridgeSync(bridge: ChatGptBridge | undefined, refresh: () => Promise<void>): boolean {
  const compact = useCompact();
  const completed = compact?.latestCompleted;
  const completionKey = completed ? `${completed.id}:${completed.revision}` : '';
  const needsSync = Boolean(completed && (completed.newConversationId
    ? bridge?.conversationId !== completed.newConversationId
    : completed.newConversationUrl && bridge?.conversationUrl !== completed.newConversationUrl));
  useEffect(() => {
    if (!completionKey) return;
    void refresh();
    if (!needsSync) return;
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [completionKey, needsSync, refresh]);
  return needsSync;
}
