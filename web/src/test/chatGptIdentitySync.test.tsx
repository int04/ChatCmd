import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import { recoverChatGptIdentity, chatGptExtensionStatus } from '../chatgptBridge';
import { ChatGptTaskCard, ChatGptTaskComposer } from '../chatgpt/ChatGptConversation';
import type { ChatGptBridge } from '../types';

vi.mock('../chatgptBridge', async (importOriginal) => ({
  ...await importOriginal<typeof import('../chatgptBridge')>(),
  recoverChatGptIdentity: vi.fn(),
  chatGptExtensionStatus: vi.fn(),
}));
vi.mock('../chatgpt/ChatGptMessageQueue', () => ({ ChatGptMessageQueuePanel: () => null }));

const missingIdentity: ChatGptBridge = {
  taskId: 'task-a', conversationId: null, conversationUrl: null, model: 'Auto',
  activeStatus: 'completed', taskStatus: 'completed',
  latestRequestId: 'request-a', latestSubmittedContent: 'xin chào',
};
const syncedIdentity: ChatGptBridge = {
  ...missingIdentity,
  conversationId: 'conversation-a',
  conversationUrl: 'https://chatgpt.com/g/g-p-test/c/conversation-a',
};

beforeEach(() => {
  vi.mocked(recoverChatGptIdentity).mockReset();
  vi.mocked(chatGptExtensionStatus).mockReset().mockResolvedValue({
    ready: true, chatGptTabOpen: true, conversationTabOpen: true, conversationReady: true,
  });
});
afterEach(() => vi.restoreAllMocks());

describe('ChatGPT identity synchronization', () => {
  it('shows the actual API rejection instead of silently spinning', async () => {
    const load = vi.spyOn(api, 'chatGptBridge').mockResolvedValue(missingIdentity);
    vi.mocked(recoverChatGptIdentity).mockRejectedValue(new Error('the ChatGPT extension cannot access this management endpoint'));
    render(<ChatGptTaskComposer taskId="task-a" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('the ChatGPT extension cannot access this management endpoint');
    await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
    expect(recoverChatGptIdentity).toHaveBeenCalledWith('request-a', 'xin chào');
  });

  it('surfaces unsuccessful recovery reasons even when extension transport succeeded', async () => {
    vi.spyOn(api, 'chatGptBridge').mockResolvedValue(missingIdentity);
    vi.mocked(recoverChatGptIdentity).mockResolvedValue({ nonce: 'n', ok: true, recovered: false, reason: 'ambiguous_match' });
    render(<ChatGptTaskComposer taskId="task-a" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('ambiguous_match');
  });

  it('rereads the backend after a recovery error and unlocks a concurrently synced conversation', async () => {
    vi.spyOn(api, 'chatGptBridge').mockResolvedValueOnce(missingIdentity).mockResolvedValue(syncedIdentity);
    vi.mocked(recoverChatGptIdentity).mockRejectedValue(new Error('extension response timed out'));
    render(<ChatGptTaskComposer taskId="task-a" />);
    expect(await screen.findByRole('textbox')).toBeEnabled();
    await waitFor(() => expect(chatGptExtensionStatus).toHaveBeenCalledWith(syncedIdentity.conversationUrl));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('updates the sidebar link when identity arrives after initial render', async () => {
    vi.useFakeTimers();
    const load = vi.spyOn(api, 'chatGptBridge').mockResolvedValueOnce(missingIdentity).mockResolvedValue(syncedIdentity);
    const { unmount } = render(<ChatGptTaskCard taskId="task-a" />);
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(screen.getByRole('link')).toHaveAttribute('href', syncedIdentity.conversationUrl);
    expect(load).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(4_000); });
    expect(load).toHaveBeenCalledTimes(2);
    unmount();
  });
});
