import { afterEach, expect, it, vi } from 'vitest';
import { dispatchSubagentFallback } from '../chatgptBridge';
import { ChatGptBridgeTimeoutError } from '../chatgpt/bridgeErrors';

afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });
const input = { subagentId: 'child', childTaskId: 'task-child', submittedContent: 'Read one file', attempt: 1 };

it('real bridge timeout exposes a distinct admission-unknown error', async () => {
  vi.useFakeTimers();
  vi.spyOn(window, 'postMessage').mockImplementation(() => {});
  const result = dispatchSubagentFallback(input).catch((error: unknown) => error);
  await vi.advanceTimersByTimeAsync(5001);
  const error = await result;
  expect(error).toBeInstanceOf(ChatGptBridgeTimeoutError);
  expect(error).toHaveProperty('code', 'bridge_ack_timeout');
});

it('real bridge accepts the matching ACK and cancels its timeout', async () => {
  vi.useFakeTimers();
  const post = vi.spyOn(window, 'postMessage').mockImplementation(() => {});
  const result = dispatchSubagentFallback(input);
  const command = post.mock.calls[0][0] as { nonce: string };
  window.dispatchEvent(new MessageEvent('message', { source: window, data: {
    type: 'chatcmd-chatgpt-extension-response', nonce: command.nonce, ok: true,
  } }));
  await expect(result).resolves.toBeUndefined();
  await vi.advanceTimersByTimeAsync(6000);
  expect(vi.getTimerCount()).toBe(0);
});
