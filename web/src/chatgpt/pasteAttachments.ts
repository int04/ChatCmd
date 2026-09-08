export const LONG_PASTE_TEXT_THRESHOLD = 8_000;

export type ChatGptTextAttachment = {
  id: string;
  name: string;
  content: string;
  mimeType: 'text/plain;charset=utf-8';
};

export type ChatGptFileAttachmentPayload = Pick<ChatGptTextAttachment, 'name' | 'content' | 'mimeType'>;

export function textAttachmentFromPaste(text: string, sequence: number): ChatGptTextAttachment | null {
  if (text.length < LONG_PASTE_TEXT_THRESHOLD || !text.trim()) return null;
  const safeSequence = Math.max(1, Math.trunc(sequence) || 1);
  return {
    id: `pasted-text-${safeSequence}`,
    name: `pasted-text-${safeSequence}.txt`,
    content: text,
    mimeType: 'text/plain;charset=utf-8',
  };
}

export function messageContentWithTextAttachments(content: string, attachments: ChatGptTextAttachment[]) {
  const message = content.trim();
  if (message || !attachments.length) return message;
  if (attachments.length === 1) return `Nội dung tin nhắn nằm trong tệp đính kèm ${attachments[0].name}.`;
  return `Nội dung tin nhắn nằm trong các tệp đính kèm: ${attachments.map((attachment) => attachment.name).join(', ')}.`;
}

export function fileAttachmentPayloads(attachments: ChatGptTextAttachment[]): ChatGptFileAttachmentPayload[] {
  return attachments.map(({ name, content, mimeType }) => ({ name, content, mimeType }));
}
