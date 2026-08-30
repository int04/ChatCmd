import type { TaskTurn, TimelineEvent } from '../types';

const MAX_FALLBACK_MATCH_DISTANCE_MS = 2 * 60_000;

export function collapseChatGptFallbackTurns(turns: TaskTurn[]): TaskTurn[] {
  return turns.filter((turn) => {
    const fallback = chatGptFallbackIdentity(turn);
    if (!fallback) return true;
    return !turns.some((candidate) => candidate.id !== turn.id && hasMatchingMcpUserMessage(candidate, fallback));
  });
}

type FallbackIdentity = { submittedContent: string; occurredAt: number };

function chatGptFallbackIdentity(turn: TaskTurn): FallbackIdentity | null {
  for (const event of turn.events ?? []) {
    if (event.type !== 'message') continue;
    const payload = payloadObject(event);
    if (stringValue(payload.role) !== 'user' || stringValue(payload.provider) !== 'chatgpt_web') continue;
    const submittedContent = stringValue(payload.submittedContent);
    if (!submittedContent) continue;
    return { submittedContent, occurredAt: Date.parse(event.occurredAt) };
  }
  return null;
}

function hasMatchingMcpUserMessage(turn: TaskTurn, fallback: FallbackIdentity) {
  return (turn.events ?? []).some((event) => {
    if (event.type !== 'message') return false;
    const payload = payloadObject(event);
    if (stringValue(payload.role) !== 'user' || stringValue(payload.provider) === 'chatgpt_web') return false;
    if (stringValue(payload.content) !== fallback.submittedContent) return false;
    const candidateAt = Date.parse(event.occurredAt);
    if (!Number.isFinite(candidateAt) || !Number.isFinite(fallback.occurredAt)) return true;
    return Math.abs(candidateAt - fallback.occurredAt) <= MAX_FALLBACK_MATCH_DISTANCE_MS;
  });
}

function payloadObject(event: TimelineEvent): Record<string, unknown> {
  return event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload)
    ? event.payload as Record<string, unknown>
    : {};
}

function stringValue(value: unknown) {
  return typeof value === 'string' && value.trim() ? value.trim() : '';
}
