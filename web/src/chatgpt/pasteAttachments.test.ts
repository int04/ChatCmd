import { describe, expect, it } from 'vitest';
import { LONG_PASTE_TEXT_THRESHOLD, clipboardAttachmentFromFile, fileAttachmentFromFile, fileAttachmentPayloads, messageContentWithTextAttachments, textAttachmentFromPaste } from './pasteAttachments';

describe('clipboard and file attachments', () => {
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

  it('converts a selected binary file to a base64 attachment', async () => {
    const file = new File([new Uint8Array([0, 1, 2, 255])], 'sample.bin', { type: 'application/octet-stream' });
    await expect(fileAttachmentFromFile(file, 3)).resolves.toEqual({
      id: 'selected-file-3',
      name: 'sample.bin',
      content: 'AAEC/w==',
      mimeType: 'application/octet-stream',
      encoding: 'base64',
      sizeBytes: 4,
    });
  });

  it('converts a pasted screenshot into a named image attachment', async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], 'image.png', { type: 'image/png' });
    await expect(clipboardAttachmentFromFile(file, 4)).resolves.toEqual({
      id: 'clipboard-file-4',
      name: 'clipboard-image-4.png',
      content: 'iVBORw==',
      mimeType: 'image/png',
      encoding: 'base64',
      sizeBytes: 4,
    });
  });

  it('uses a small textual prompt when the message consists only of attachments', () => {
    const attachment = textAttachmentFromPaste('x'.repeat(LONG_PASTE_TEXT_THRESHOLD), 1)!;
    expect(messageContentWithTextAttachments('', [attachment]))
      .toBe('Nội dung tin nhắn nằm trong tệp đính kèm pasted-text-1.txt.');
    expect(messageContentWithTextAttachments('  xem giúp nội dung này  ', [attachment]))
      .toBe('xem giúp nội dung này');
  });

  it('strips UI-only ids while preserving attachment encoding in the bridge payload', () => {
    const textAttachment = textAttachmentFromPaste('x'.repeat(LONG_PASTE_TEXT_THRESHOLD), 1)!;
    const binaryAttachment = {
      id: 'selected-file-1',
      name: 'sample.bin',
      content: 'AAEC/w==',
      mimeType: 'application/octet-stream',
      encoding: 'base64' as const,
      sizeBytes: 4,
    };
    expect(fileAttachmentPayloads([textAttachment, binaryAttachment])).toEqual([
      {
        name: 'pasted-text-1.txt',
        content: textAttachment.content,
        mimeType: 'text/plain;charset=utf-8',
      },
      {
        name: 'sample.bin',
        content: 'AAEC/w==',
        mimeType: 'application/octet-stream',
        encoding: 'base64',
      },
    ]);
  });
});
