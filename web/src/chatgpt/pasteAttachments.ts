export const LONG_PASTE_TEXT_THRESHOLD = 8_000;

export type ChatGptAttachmentEncoding = 'utf8' | 'base64';

export type ChatGptTextAttachment = {
  id: string;
  name: string;
  content: string;
  mimeType: string;
  encoding?: ChatGptAttachmentEncoding;
  sizeBytes?: number;
};

export type ChatGptFileAttachmentPayload = Pick<ChatGptTextAttachment, 'name' | 'content' | 'mimeType' | 'encoding'>;

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

export async function fileAttachmentFromFile(file: File, sequence: number): Promise<ChatGptTextAttachment> {
  const safeSequence = safeAttachmentSequence(sequence);
  return binaryAttachmentFromFile(file, `selected-file-${safeSequence}`, file.name.trim() || `attachment-${safeSequence}`);
}

export async function clipboardAttachmentFromFile(file: File, sequence: number): Promise<ChatGptTextAttachment> {
  const safeSequence = safeAttachmentSequence(sequence);
  const name = clipboardAttachmentName(file, safeSequence);
  return binaryAttachmentFromFile(file, `clipboard-file-${safeSequence}`, name);
}

export function messageContentWithTextAttachments(content: string, attachments: ChatGptTextAttachment[]) {
  const message = content.trim();
  if (message || !attachments.length) return message;
  if (attachments.length === 1) return `Nội dung tin nhắn nằm trong tệp đính kèm ${attachments[0].name}.`;
  return `Nội dung tin nhắn nằm trong các tệp đính kèm: ${attachments.map((attachment) => attachment.name).join(', ')}.`;
}

export function fileAttachmentPayloads(attachments: ChatGptTextAttachment[]): ChatGptFileAttachmentPayload[] {
  return attachments.map(({ name, content, mimeType, encoding }) => ({ name, content, mimeType, ...(encoding ? { encoding } : {}) }));
}

async function binaryAttachmentFromFile(file: File, id: string, name: string): Promise<ChatGptTextAttachment> {
  return {
    id,
    name,
    content: await fileAsBase64(file),
    mimeType: file.type || 'application/octet-stream',
    encoding: 'base64',
    sizeBytes: file.size,
  };
}

function safeAttachmentSequence(sequence: number) {
  return Math.max(1, Math.trunc(sequence) || 1);
}

function clipboardAttachmentName(file: File, sequence: number) {
  if (!file.type.toLowerCase().startsWith('image/')) return file.name.trim() || `clipboard-file-${sequence}`;
  return `clipboard-image-${sequence}.${imageExtension(file.type)}`;
}

function imageExtension(mimeType: string) {
  switch (mimeType.toLowerCase()) {
    case 'image/jpeg': return 'jpg';
    case 'image/gif': return 'gif';
    case 'image/webp': return 'webp';
    case 'image/bmp': return 'bmp';
    case 'image/svg+xml': return 'svg';
    case 'image/avif': return 'avif';
    default: return 'png';
  }
}

function fileAsBase64(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Could not read file.'));
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== 'string') {
        reject(new Error('Could not read file.'));
        return;
      }
      const comma = result.indexOf(',');
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}
