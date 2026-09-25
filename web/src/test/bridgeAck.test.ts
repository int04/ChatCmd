import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { dispatchSubagentFallback } from '../chatgptBridge';
import { ChatGptBridgeTimeoutError } from '../chatgpt/bridgeErrors';

const request = { subagentId: 'child', childTaskId: 'task-child', submittedContent: 'Read one file', attempt: 1 };
beforeEach(() => { vi.useFakeTimers(); vi.spyOn(window, 'postMessage').mockImplementation(() => {}); });
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

it('real bridge timeout is typed as unknown acknowledgement, not execution failure', async () => {
  const pending = dispatchSubagentFallback(request).catch((error: unknown) => error);
  await vi.advanceTimersByTimeAsync(5_001);
  const error = await pending;
  expect(error).toBeInstanceOf(ChatGptBridgeTimeoutError);
  expect((error as ChatGptBridgeTimeoutError).code).toBe('bridge_ack_timeout');
});

it('only the matching admission ACK resolves the requested child', async () => {
  let resolved = false;
  const pending = dispatchSubagentFallback(request).then(() => { resolved = true; });
  const command = vi.mocked(window.postMessage).mock.calls[0][0] as { nonce: string };
  const ack = (nonce: string) => window.dispatchEvent(new MessageEvent('message', {
    source: window, data: { type: 'chatcmd-chatgpt-extension-response', nonce, ok: true, accepted: true },
  }));
  ack('another-child');
  await Promise.resolve();
  expect(resolved).toBe(false);
  ack(command.nonce);
  await pending;
  expect(resolved).toBe(true);
  await vi.advanceTimersByTimeAsync(5_001);
});
