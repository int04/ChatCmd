import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import type { Task, TimelineEvent } from '../types';
import { TurnThinkingSources } from './TurnThinkingSources';
import { browserThinking } from './chatGptThinking';
import { mergeTimelineEvents, realtimeEventKey } from './timelineSnapshots';
import { buildTaskTurns, findFinalResponse, findUserMessage, mergeLiveDetail, mergeTaskEvent } from './taskTimeline';

afterEach(cleanup);
const task: Task = { id: 'task-a', status: 'running', source: 'chatgpt_web', updatedAtUtc: '2026-09-05T12:00:00Z' };
const event = (id: string, turnId: string, type: string, payload: unknown): TimelineEvent => ({
  id, turnId, type, payload, taskId: task.id, occurredAt: '2026-09-05T12:00:01Z',
});
const snapshot = (revision: number, content: string) => event('chatgpt-think-request-a', 'browser-turn', 'chatgpt_think', {
  provider: 'chatgpt_web', bridgeRequestId: 'request-a', revision, completed: false,
  messages: [{ id: 'message-1', kind: 'commentary', content }],
});
const user = event('browser-user', 'browser-turn', 'message', {
  role: 'user', content: 'Hello', submittedContent: 'Use plugin: Hello', provider: 'chatgpt_web', bridgeRequestId: 'request-a',
});
const mcpUser = event('mcp-user', 'mcp-turn', 'message', {
  role: 'user', content: 'Use plugin: Hello', tool: 'agent_user_message', bridgeRequestId: 'request-a', browserTurnId: 'browser-turn',
});

describe('request-scoped browser snapshots', () => {
  it('updates the same event id and rejects stale API or websocket snapshots', () => {
    expect(mergeTimelineEvents([snapshot(1, 'Partial')], [snapshot(3, 'Latest'), snapshot(2, 'Stale')])).toEqual([snapshot(3, 'Latest')]);
    expect(realtimeEventKey(snapshot(1, 'Partial'))).not.toBe(realtimeEventKey(snapshot(2, 'Latest')));
    const detail = mergeLiveDetail({ task, events: [user, snapshot(3, 'Saved latest')] }, [snapshot(1, 'Old live')]);
    expect(browserThinking(detail.events ?? []).messages[0].content).toBe('Saved latest');
  });
  it('keeps a standalone basic Q&A and does not infer completion from thought snapshots', () => {
    const turns = buildTaskTurns([user, snapshot(1, 'Visible answer')], task);
    expect(turns).toHaveLength(1);
    expect(turns[0].status).toBe('running');
    const done = { ...task, status: 'completed' };
    expect(mergeTaskEvent(done, snapshot(2, 'Delayed text'))).toBe(done);
  });
  it('merges browser and MCP into one bubble while retaining both sources', () => {
    const progress = event('mcp-progress', 'mcp-turn', 'progress', { message: 'MCP action' });
    const turns = buildTaskTurns([user, snapshot(1, 'Visible ChatGPT'), mcpUser, progress], task);
    expect(turns).toHaveLength(1);
    expect(turns[0].id).toBe('mcp-turn');
    expect(browserThinking(turns[0].events ?? []).messages[0].content).toBe('Visible ChatGPT');
    expect(turns[0].events?.some((item) => item.id === progress.id)).toBe(true);
    expect(findUserMessage(turns[0].events ?? [])?.event.id).toBe(mcpUser.id);
  });
  it('prefers the MCP final even if a browser final arrives later', () => {
    const mcp = event('mcp-final', 'mcp-turn', 'status', { status: 'completed', content: 'MCP final' });
    const browser = event('browser-final', 'mcp-turn', 'status', { provider: 'chatgpt_web', status: 'completed', content: 'Browser final' });
    expect(findFinalResponse([mcp, browser])?.text).toBe('MCP final');
    expect(browserThinking([mcp, browser]).messages[0].content).toBe('Browser final');
  });
  it('does not merge equal repeated prompts when the legacy association is ambiguous', () => {
    const older = { ...user, id: 'older-user', turnId: 'older-browser-turn' };
    const legacy = { ...mcpUser, payload: { role: 'user', content: 'Use plugin: Hello', tool: 'agent_user_message' } };
    expect(buildTaskTurns([older, user, legacy], task)).toHaveLength(3);
  });
});

describe('thinking source selection', () => {
  const browser = browserThinking([snapshot(1, 'Visible ChatGPT commentary')]);
  const view = (hasMcp: boolean) => <TurnThinkingSources browser={browser} hasMcp={hasMcp} running><p>MCP tools and progress</p></TurnThinkingSources>;
  it('shows ChatGPT before MCP, switches once on arrival and allows returning to retained ChatGPT', () => {
    const { rerender } = render(view(false));
    expect(screen.getByRole('button', { name: /ChatGPT Think/ })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /ChatCMD Think/ })).toBeDisabled();
    expect(screen.getByText('Visible ChatGPT commentary')).toBeInTheDocument();
    rerender(view(true));
    expect(screen.getByRole('button', { name: /ChatCMD Think/ })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByText('MCP tools and progress')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /ChatGPT Think/ }));
    rerender(view(true));
    expect(screen.getByText('Visible ChatGPT commentary')).toBeInTheDocument();
    expect(screen.queryByText('MCP tools and progress')).not.toBeInTheDocument();
  });
  it('retains standalone ChatGPT answers when a completed turn is opened from history', () => {
    render(<TurnThinkingSources browser={{ ...browser, completed: true }} hasMcp={false} running={false}>No MCP</TurnThinkingSources>);
    expect(within(screen.getByRole('region', { name: 'ChatGPT Think' })).getByText('Visible ChatGPT commentary')).toBeInTheDocument();
  });
  it('renders browser content as sanitized Markdown, never executable page HTML', () => {
    const { container } = render(<TurnThinkingSources browser={{ ...browser, messages: [{ id: 'a', kind: 'answer', content: '<script>alert(1)</script>\n\n[unsafe](javascript:alert(1))\n\n**Safe text**' }] }} hasMcp={false} running={false}>MCP</TurnThinkingSources>);
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('a[href^="javascript:"]')).toBeNull();
    expect(screen.getByText('Safe text').tagName).toBe('STRONG');
  });
});
