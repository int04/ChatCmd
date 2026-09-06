import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import type { TimelineEvent } from '../../types';
import { TaskTurnBubble } from '../TaskTurnBubble';
import { TurnThinkingSources } from '../TurnThinkingSources';

const writing = ':::writing{variant="document" id="58321" title="Một chút dịu dàng"} Chiều nghiêng qua cửa rất êm. :::';
const event = (id: string, type: string, payload: unknown): TimelineEvent => ({
  id, type, payload, taskId: 'task-chat-test', turnId: 'turn-test', occurredAt: '2026-09-06T01:00:00Z',
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('task chat shared rich-text integration', () => {
  it('renders a historical user message and final reply through the real task bubble', () => {
    const { container } = render(<TaskTurnBubble taskId="task-chat-test" agentLabel="ChatGPT" turn={{
      id: 'turn-test', status: 'completed', events: [
        event('user', 'message', { role: 'user', content: writing }),
        event('final', 'status', { status: 'completed', content: writing }),
      ],
    }} />);
    expect(container.querySelector('.turn-user-content .chat-writing-card')).toHaveTextContent('Một chút dịu dàng');
    expect(container.querySelector('.turn-response-content .chat-writing-card')).toHaveTextContent('Chiều nghiêng qua cửa rất êm.');
    expect(screen.getAllByRole('region', { name: 'Một chút dịu dàng' })).toHaveLength(2);
    expect(container.textContent).not.toContain(':::writing');
  });
  it('uses the same renderer for MCP progress while a turn is running', () => {
    vi.spyOn(window, 'requestAnimationFrame').mockReturnValue(0);
    const { container } = render(<TaskTurnBubble taskId="task-chat-test" agentLabel="ChatGPT" turn={{
      id: 'turn-test', status: 'running', events: [
        event('user', 'message', { role: 'user', content: 'Write a poem' }),
        event('progress', 'progress', { message: writing }),
      ],
    }} />);
    expect(container.querySelector('.turn-progress-content .chat-writing-card')).toHaveTextContent('Một chút dịu dàng');
    expect(container.textContent).not.toContain(':::writing');
  });
  it('uses the same renderer for live and saved browser snapshots', () => {
    const view = (completed: boolean, content: string) => <TurnThinkingSources hasMcp={false} running={!completed}
      browser={{ completed, revision: completed ? 2 : 1, messages: [{ id: 'browser-message', kind: 'answer', content }] }}>MCP</TurnThinkingSources>;
    const { rerender } = render(view(false, writing.slice(0, -4)));
    const browser = screen.getByRole('region', { name: 'ChatGPT Think' });
    expect(within(browser).getByRole('region', { name: 'Một chút dịu dàng' })).toBeInTheDocument();
    rerender(view(true, writing));
    expect(within(browser).getAllByRole('region', { name: 'Một chút dịu dàng' })).toHaveLength(1);
    expect(browser).not.toHaveTextContent(':::writing');
  });
});
