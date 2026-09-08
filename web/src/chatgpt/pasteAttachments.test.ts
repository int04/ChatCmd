import { describe, expect, it } from 'vitest';
import { LONG_PASTE_TEXT_THRESHOLD, fileAttachmentPayloads, messageContentWithTextAttachments, textAttachmentFromPaste } from './pasteAttachments';

describe('clipboard text attachments', () => {
  it('keeps short clipboard text in the textarea', () => {
    expect(textAttachmentFromPaste('a'.repeat(LONG_PASTE_TEXT_THRESHOLD - 1), 1)).toBeNull();
  });

  it('turns a long paste into a UTF-8 txt attachment without changing its content', () => {
    const text = `Đầu file\n${'x'.repeat(LONG_PASTE_TEXT_THRESHOLD)}`;
    expect(textAttachmentFromPaste(text, 2)).toEqual({
      id: 'pasted-text-2',
      name: 'pasted-text-2.txt',
      content: text,
      mimeType: 'text/plain;charset=utf-8',
    });
  });

  it('uses a small textual prompt when the message consists only of attachments', () => {
    const attachment = textAttachmentFromPaste('x'.repeat(LONG_PASTE_TEXT_THRESHOLD), 1)!;
    expect(messageContentWithTextAttachments('', [attachment]))
      .toBe('Nội dung tin nhắn nằm trong tệp đính kèm pasted-text-1.txt.');
    expect(messageContentWithTextAttachments('  xem giúp nội dung này  ', [attachment]))
      .toBe('xem giúp nội dung này');
  });

  it('strips UI-only ids from the bridge payload', () => {
    const attachment = textAttachmentFromPaste('x'.repeat(LONG_PASTE_TEXT_THRESHOLD), 1)!;
    expect(fileAttachmentPayloads([attachment])).toEqual([{
      name: 'pasted-text-1.txt',
      content: attachment.content,
      mimeType: 'text/plain;charset=utf-8',
    }]);
  });
});
