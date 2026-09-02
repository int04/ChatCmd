import { useCallback, useEffect, useRef } from 'react';
import { api, type SubagentFallbackRequest } from '../api';
import { closeSubagentFallbackTab, dispatchSubagentFallback } from '../chatgptBridge';
import { useRealtime } from '../realtime';
import type { TimelineEvent } from '../types';

export function GlobalSubagentFallbackBridge() {
  const inFlight = useRef(new Set<string>());

  const dispatchFallback = useCallback(async (fallback: SubagentFallbackRequest) => {
    if (!isDispatchable(fallback)) return;
    const key = `${fallback.subagentId}:${fallback.attempt}`;
    if (inFlight.current.has(key)) return;
    inFlight.current.add(key);
    try {
      await dispatchSubagentFallback({
        subagentId: fallback.subagentId,
        childTaskId: fallback.childTaskId,
        submittedContent: fallback.submittedContent,
        attempt: fallback.attempt,
      });
    } catch (error) {
      try {
        await api.reportSubagentFallbackResult(fallback.subagentId, {
          attempt: fallback.attempt,
          status: 'failed',
          errorMessage: error instanceof Error ? error.message : String(error),
        });
      } catch {
        // The pending endpoint will recover this attempt after the local API/realtime reconnects.
      }
    } finally {
      inFlight.current.delete(key);
    }
  }, []);

  const recoverPending = useCallback(async () => {
    try {
      const pending = await api.pendingSubagentFallbacks();
      await Promise.all(pending.map(dispatchFallback));
    } catch {
      // A later realtime reconnect will try recovery again.
    }
  }, [dispatchFallback]);

  const realtimeState = useRealtime(useCallback((event: TimelineEvent) => {
    const payload = record(event.payload);
    if (event.type === 'subagent.fallback_requested') {
      const fallback = fallbackFromPayload(payload);
      if (fallback) void dispatchFallback(fallback);
      return;
    }
    if (event.type === 'subagent.fallback_claimed') {
      const subagentId = stringValue(payload.subagentId);
      if (subagentId) void closeSubagentFallbackTab(subagentId).catch(() => undefined);
    }
  }, [dispatchFallback]));

  useEffect(() => {
    if (realtimeState === 'online') void recoverPending();
  }, [realtimeState, recoverPending]);

  return null;
}

function fallbackFromPayload(payload: Record<string, unknown>): SubagentFallbackRequest | null {
  const subagentId = stringValue(payload.subagentId);
  const childTaskId = stringValue(payload.childTaskId);
  const submittedContent = stringValue(payload.submittedContent);
  const attempt = positiveInteger(payload.attempt);
  if (!subagentId || !childTaskId || !submittedContent || !attempt) return null;
  return {
    subagentId,
    childTaskId,
    submittedContent,
    attempt,
    maxAttempts: positiveInteger(payload.maxAttempts) ?? 3,
    parentTaskId: stringValue(payload.parentTaskId) || undefined,
    parentTurnId: stringValue(payload.parentTurnId) || undefined,
    name: stringValue(payload.name) || 'Sub-agent',
    conversationId: stringValue(payload.conversationId) || undefined,
    conversationUrl: stringValue(payload.conversationUrl) || undefined,
  };
}

function isDispatchable(value: SubagentFallbackRequest) {
  return Boolean(
    value.subagentId.trim()
      && value.childTaskId.trim()
      && value.submittedContent.trim()
      && Number.isInteger(value.attempt)
      && value.attempt > 0,
  );
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function stringValue(value: unknown) {
  return typeof value === 'string' ? value.trim() : '';
}

function positiveInteger(value: unknown) {
  const number = Number(value);
  return Number.isInteger(number) && number > 0 ? number : null;
}
