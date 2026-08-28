import { describe, expect, it } from 'vitest';
import { isFinalResponseEvent, isNewConversationEvent } from '../App';

const event = (type: string, payload: unknown) => ({ id: `${type}-event`, type, occurredAt: '2026-08-28T00:00:00.000Z', taskId: 'task-1', payload });

describe('sound notification events', () => {
  it('recognizes only the first titled user message as a new conversation event', () => {
    expect(isNewConversationEvent(event('message', { role: 'user', title: 'New task', content: 'hello' }))).toBe(true);
    expect(isNewConversationEvent(event('message', { role: 'user', content: 'follow up' }))).toBe(false);
    expect(isNewConversationEvent(event('message', { role: 'assistant', title: 'New task' }))).toBe(false);
  });

  it('plays completion semantics only when completed status contains a final response', () => {
    expect(isFinalResponseEvent(event('status', { status: 'completed', content: 'Phản hồi cuối' }))).toBe(true);
    expect(isFinalResponseEvent(event('status', { status: 'completed' }))).toBe(false);
    expect(isFinalResponseEvent(event('status', { status: 'running', content: 'progress' }))).toBe(false);
  });
});
