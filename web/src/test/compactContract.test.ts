import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import { decodeEncryptedApiResponse, encryptedApiFetch } from '../apiCrypto';
import { resumeChatGptCompact } from '../chatgptBridge';
import { compactReferenceUrl } from '../chatgpt/compact/types';
import { compactJob, compactTaskId, oldUrl } from './compactFixtures';

vi.mock('../apiCrypto', () => ({ encryptedApiFetch: vi.fn(), decodeEncryptedApiResponse: vi.fn() }));
beforeEach(() => {
  vi.mocked(encryptedApiFetch).mockReset().mockResolvedValue(new Response('{}', { status: 200 }));
  vi.mocked(decodeEncryptedApiResponse).mockReset().mockResolvedValue(compactJob());
});
afterEach(() => vi.restoreAllMocks());

describe('compact local API and extension contract', () => {
  it('routes all four operations through the encrypted local API wrapper with encoded identifiers', async () => {
    const taskId = 'task/a ?'; const jobId = 'job/b ?';
    await api.chatGptCompact(taskId);
    await api.startChatGptCompact(taskId);
    await api.chatGptCompactJob(jobId);
    await api.cancelChatGptCompact(jobId, 7);
    expect(encryptedApiFetch).toHaveBeenNthCalledWith(1, '/api/local/tasks/task%2Fa%20%3F/chatgpt/compact', {});
    expect(encryptedApiFetch).toHaveBeenNthCalledWith(2, '/api/local/tasks/task%2Fa%20%3F/chatgpt/compact', { method: 'POST', body: JSON.stringify({ continueAfterCompact: false }) });
    expect(encryptedApiFetch).toHaveBeenNthCalledWith(3, '/api/local/chatgpt/compact/job%2Fb%20%3F', {});
    expect(encryptedApiFetch).toHaveBeenNthCalledWith(4, '/api/local/chatgpt/compact/job%2Fb%20%3F/checkpoint', {
      method: 'POST', body: JSON.stringify({ expectedRevision: 7, phase: 'cancelled' }),
    });
    expect(decodeEncryptedApiResponse).toHaveBeenCalledTimes(4);
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
