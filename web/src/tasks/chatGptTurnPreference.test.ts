import { describe, expect, it } from 'vitest';
import type { TaskTurn, TimelineEvent } from '../types';
import { collapseChatGptFallbackTurns } from './chatGptTurnPreference';

const baseTime = Date.parse('2026-08-30T14:45:00.000Z');

function event(id: string, turnId: string, offsetMs: number, payload: Record<string, unknown>): TimelineEvent {
  return {
    id,
    type: 'message',
    turnId,
    occurredAt: new Date(baseTime + offsetMs).toISOString(),
    payload,
  };
}

function turn(id: string, events: TimelineEvent[]): TaskTurn {
  return { id, status: 'completed', events };
}

describe('collapseChatGptFallbackTurns', () => {
  it('prefers the MCP turn over a matching ChatGPT fallback turn', () => {
    const submitted = 'Sử dụng plugin @test_rust để thực hiện yêu cầu sau:\n\ncommit';
    const fallback = turn('chatgpt-turn-fallback', [event('fallback-user', 'chatgpt-turn-fallback', 0, {
      role: 'user',
      content: 'commit',
      submittedContent: submitted,
      provider: 'chatgpt_web',
    })]);
    const mcp = turn('turn-mcp', [event('mcp-user', 'turn-mcp', 10_000, {
      role: 'user',
      content: submitted,
      tool: 'agent_user_message',
    })]);

    expect(collapseChatGptFallbackTurns([fallback, mcp]).map((item) => item.id)).toEqual(['turn-mcp']);
  });

  it('keeps the ChatGPT fallback when no MCP turn exists', () => {
    const fallback = turn('chatgpt-turn-fallback', [event('fallback-user', 'chatgpt-turn-fallback', 0, {
      role: 'user',
      content: 'xin chào',
      submittedContent: 'xin chào',
      provider: 'chatgpt_web',
    })]);

    expect(collapseChatGptFallbackTurns([fallback]).map((item) => item.id)).toEqual(['chatgpt-turn-fallback']);
  });

  it('does not collapse an unrelated later MCP turn with the same text', () => {
    const submitted = 'Sử dụng plugin @test_rust để thực hiện yêu cầu sau:\n\ncommit';
    const fallback = turn('chatgpt-turn-fallback', [event('fallback-user', 'chatgpt-turn-fallback', 0, {
      role: 'user',
      content: 'commit',
      submittedContent: submitted,
      provider: 'chatgpt_web',
    })]);
    const laterMcp = turn('turn-mcp-later', [event('mcp-user-later', 'turn-mcp-later', 3 * 60_000, {
      role: 'user',
      content: submitted,
      tool: 'agent_user_message',
    })]);

    expect(collapseChatGptFallbackTurns([fallback, laterMcp]).map((item) => item.id)).toEqual([
      'chatgpt-turn-fallback',
      'turn-mcp-later',
    ]);
  });
});
