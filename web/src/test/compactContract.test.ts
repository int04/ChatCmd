import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import { resumeChatGptCompact } from '../chatgptBridge';
import { compactReferenceUrl } from '../chatgpt/compact/types';
import { compactJob, compactTaskId, oldUrl } from './compactFixtures';

afterEach(() => vi.restoreAllMocks());

describe('compact local API and extension contract', () => {
  it('routes all four operations through plaintext JSON fetch with encoded identifiers', async () => {
    const fetchMock = vi.fn((_path: string | URL | Request, _init?: RequestInit) => Promise.resolve(new Response(JSON.stringify(compactJob()), { status: 200, headers: { 'Content-Type': 'application/json' } })));
    vi.stubGlobal('fetch', fetchMock);
    const taskId = 'task/a ?'; const jobId = 'job/b ?';
    await api.chatGptCompact(taskId);
    await api.startChatGptCompact(taskId);
    await api.chatGptCompactJob(jobId);
    await api.cancelChatGptCompact(jobId, 7);
    expect(fetchMock.mock.calls.map(([path]) => String(path))).toEqual([
      '/api/local/tasks/task%2Fa%20%3F/chatgpt/compact',
      '/api/local/tasks/task%2Fa%20%3F/chatgpt/compact',
      '/api/local/chatgpt/compact/job%2Fb%20%3F',
      '/api/local/chatgpt/compact/job%2Fb%20%3F/checkpoint',
    ]);
    for (const [, init] of fetchMock.mock.calls) {
      const headers = new Headers(init?.headers);
      expect(headers.get('X-ChatCmdClient')).toBe('local-ui');
      expect(headers.has('X-ChatCmd-Crypto')).toBe(false);
      expect(headers.has('X-ChatCmd-Crypto-Session')).toBe(false);
    }
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ method: 'POST', body: JSON.stringify({ continueAfterCompact: false }) });
    expect(fetchMock.mock.calls[3]?.[1]).toMatchObject({ method: 'POST', body: JSON.stringify({ expectedRevision: 7, phase: 'cancelled' }) });
  });

  it('posts compact-resume with durable job/task ids and the existing local origin bridge envelope', async () => {
    const post = vi.spyOn(window, 'postMessage').mockImplementation((message: unknown) => {
      const data = message as { nonce: string };
      window.dispatchEvent(new MessageEvent('message', { source: window, origin: window.location.origin,
        data: { type: 'chatcmd-chatgpt-extension-response', nonce: data.nonce, ok: true } }));
    });
    await resumeChatGptCompact('compact-test-job', compactTaskId);
    expect(post).toHaveBeenCalledExactlyOnceWith({
      type: 'chatcmd-chatgpt-extension-request', action: 'compact-resume', nonce: expect.any(String),
      jobId: 'compact-test-job', taskId: compactTaskId, localBaseUrl: window.location.origin,
    }, window.location.origin);
  });

  it('rejects an unavailable extension rather than claiming the job completed', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'postMessage').mockImplementation(() => undefined);
    const wake = resumeChatGptCompact('compact-test-job', compactTaskId);
    const assertion = expect(wake).rejects.toThrow(/respond|phản hồi/i);
    await vi.advanceTimersByTimeAsync(5_000);
    await assertion;
  });

  it('ignores responses with an unrelated nonce', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'postMessage').mockImplementation(() => {
      window.dispatchEvent(new MessageEvent('message', { source: window,
        data: { type: 'chatcmd-chatgpt-extension-response', nonce: 'not-our-request', ok: true } }));
    });
    const assertion = expect(resumeChatGptCompact('compact-test-job', compactTaskId)).rejects.toThrow();
    await vi.advanceTimersByTimeAsync(5_000); await assertion;
  });

  it.each(['javascript:alert(1)', 'https://evil.example/c/old', 'https://chatgpt.com.evil.example/c/old', 'https://user@chatgpt.com/c/old', 'http://chatgpt.com/c/old', 'https://chatgpt.com/'])('does not expose an unsafe reference URL: %s', (url) => {
    expect(compactReferenceUrl(url)).toBeUndefined();
  });
  it('accepts the old project conversation URL unchanged', () => expect(compactReferenceUrl(oldUrl)).toBe(oldUrl));
});
