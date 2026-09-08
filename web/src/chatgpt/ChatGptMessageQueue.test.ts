import { describe, expect, it } from 'vitest';
import { CHATGPT_QUEUE_TEXT_LIMIT, clampChatGptQueueDraft } from './ChatGptMessageQueue';
import { LONG_PASTE_TEXT_THRESHOLD } from './pasteAttachments';

describe('ChatGPT queue popup text limit', () => {
  it('stays below the long-paste attachment threshold', () => {
    expect(CHATGPT_QUEUE_TEXT_LIMIT).toBe(LONG_PASTE_TEXT_THRESHOLD - 1);
    expect(CHATGPT_QUEUE_TEXT_LIMIT).toBe(7_999);
  });

  it('keeps text inside the allowed range unchanged', () => {
    const value = 'a'.repeat(CHATGPT_QUEUE_TEXT_LIMIT);
    expect(clampChatGptQueueDraft(value)).toBe(value);
  });

  it('truncates text that exceeds the popup limit', () => {
    const value = 'a'.repeat(CHATGPT_QUEUE_TEXT_LIMIT + 25);
    expect(clampChatGptQueueDraft(value)).toHaveLength(CHATGPT_QUEUE_TEXT_LIMIT);
  });
});
