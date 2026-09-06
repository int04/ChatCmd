import type { TimelineEvent } from '../types';
import { snapshotRevision } from './timelineSnapshots';

export type BrowserThought = { id: string; kind: 'commentary' | 'answer'; content: string };
export type BrowserThinking = { messages: BrowserThought[]; completed: boolean; revision: number };
export function isBrowserEvent(event: TimelineEvent): boolean {
  return event.type === 'chatgpt_think' || object(event.payload).provider === 'chatgpt_web';
}
export function browserThinking(events: TimelineEvent[]): BrowserThinking {
  const snapshot = events.filter((event) => event.type === 'chatgpt_think')
    .sort((left, right) => snapshotRevision(right) - snapshotRevision(left))[0];
  const payload = object(snapshot?.payload);
  const messages: BrowserThought[] = Array.isArray(payload.messages) ? payload.messages.flatMap((item) => {
    const value = object(item);
    if (typeof value.id !== 'string' || typeof value.content !== 'string' || !value.content.trim()) return [];
    return [{ id: value.id, kind: value.kind === 'commentary' ? 'commentary' as const : 'answer' as const, content: value.content }];
  }) : [];
  // Older saved turns have only a browser-completed status, but are still a ChatGPT source.
  const final = [...events].reverse().find((event) => isBrowserEvent(event) && event.type === 'status' && object(event.payload).status === 'completed');
  const content = object(final?.payload).content;
  if (!messages.length && typeof content === 'string' && content.trim()) {
    messages.push({ id: final!.id, kind: 'answer', content });
  }
  return { messages, completed: payload.completed === true || Boolean(final), revision: snapshot ? snapshotRevision(snapshot) : 0 };
}
function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
