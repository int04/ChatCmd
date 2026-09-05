import type { TimelineEvent } from '../types';

export function snapshotRevision(event: TimelineEvent): number {
  if (event.type !== 'chatgpt_think' || !event.payload || typeof event.payload !== 'object') return 0;
  const revision = (event.payload as Record<string, unknown>).revision;
  return typeof revision === 'number' && Number.isSafeInteger(revision) ? revision : 0;
}

export function realtimeEventKey(event: TimelineEvent): string {
  return event.type === 'chatgpt_think' ? `${event.id}:${snapshotRevision(event)}:${event.turnId ?? ''}` : event.id;
}

/** A browser stream replaces one bounded record, never appends a copy per token. */
export function mergeTimelineEvents(...groups: TimelineEvent[][]): TimelineEvent[] {
  const byId = new Map<string, TimelineEvent>();
  for (const events of groups) for (const event of events) {
    const old = byId.get(event.id);
    if (!old || (event.type === 'chatgpt_think' && snapshotRevision(event) > snapshotRevision(old))) {
      byId.set(event.id, event.type === 'chatgpt_think' ? withRequestTime(event) : event);
    }
  }
  return [...byId.values()].sort((left, right) => Date.parse(left.occurredAt) - Date.parse(right.occurredAt) || left.id.localeCompare(right.id));
}

function withRequestTime(event: TimelineEvent): TimelineEvent {
  const payload = event.payload as Record<string, unknown> | undefined;
  const createdAt = payload?.requestCreatedAtMs;
  return typeof createdAt === 'number' && Number.isFinite(createdAt)
    ? { ...event, occurredAt: new Date(createdAt + 1).toISOString() } : event;
}
