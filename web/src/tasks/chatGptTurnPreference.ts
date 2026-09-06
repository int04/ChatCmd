import type { TaskTurn, TimelineEvent } from '../types';
import { mergeTimelineEvents } from './timelineSnapshots';

const MAX_FALLBACK_MATCH_DISTANCE_MS = 2 * 60_000;
const payloadOf = (event: TimelineEvent): Record<string, unknown> =>
  event.payload && typeof event.payload === 'object' ? event.payload as Record<string, unknown> : {};
const isUser = (event: TimelineEvent) => event.type === 'message' && payloadOf(event).role === 'user';

/** Merge sources, not discard fallback turns. Server-owned links win over legacy text matching. */
export function collapseChatGptFallbackTurns(turns: TaskTurn[]): TaskTurn[] {
  const aliases = new Map<string, string>();
  const byId = new Map(turns.map((turn) => [turn.id, { ...turn, events: [...(turn.events ?? [])] }]));
  for (const turn of turns) for (const event of turn.events ?? []) {
    const payload = payloadOf(event);
    if (isUser(event) && payload.provider !== 'chatgpt_web' && typeof payload.browserTurnId === 'string') {
      if (payload.browserTurnId !== turn.id) aliases.set(payload.browserTurnId, turn.id);
    }
  }
  // Backwards compatibility for saved turns from older clients, only when unambiguous.
  const proposals = new Map<string, string[]>();
  for (const turn of turns) {
    if (aliases.has(turn.id)) continue;
    const fallback = (turn.events ?? []).find((event) => isUser(event) && payloadOf(event).provider === 'chatgpt_web');
    if (!fallback || (turn.events ?? []).some((event) => isUser(event) && payloadOf(event).provider !== 'chatgpt_web')) continue;
    const prompt = payloadOf(fallback).submittedContent;
    if (typeof prompt !== 'string') continue;
    const candidates = turns.filter((candidate) => candidate.id !== turn.id && (candidate.events ?? []).some((event) => {
      if (!isUser(event)) return false;
      const payload = payloadOf(event);
      if (payload.provider === 'chatgpt_web' || payload.content !== prompt) return false;
      if (typeof payload.bridgeRequestId === 'string') return payload.bridgeRequestId === payloadOf(fallback).bridgeRequestId;
      const distance = Date.parse(event.occurredAt) - Date.parse(fallback.occurredAt);
      return Number.isFinite(distance) && distance >= 0 && distance <= MAX_FALLBACK_MATCH_DISTANCE_MS;
    }));
    if (candidates.length === 1) {
      const target = candidates[0].id;
      proposals.set(target, [...(proposals.get(target) ?? []), turn.id]);
    }
  }
  for (const [target, sources] of proposals) if (sources.length === 1) aliases.set(sources[0], target);
  for (const [source, target] of aliases) {
    const fallback = byId.get(source);
    const mcp = byId.get(target);
    if (!fallback || !mcp) continue;
    const browserEvents = fallback.events.filter((event) => !isUser(event))
      .map((event) => ({ ...event, turnId: target }));
    mcp.events = mergeTimelineEvents(mcp.events, browserEvents);
    byId.delete(source);
  }
  return [...byId.values()];
}
