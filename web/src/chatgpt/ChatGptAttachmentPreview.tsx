import {
  File,
  FileArchive,
  FileAudio2,
  FileCode2,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileType2,
  FileVideo2,
  Presentation,
  X,
} from 'lucide-react';
import { useState } from 'react';
import { createPortal } from 'react-dom';

import { Modal } from '../components';
import { tr } from '../i18n';
import type { ChatGptTextAttachment } from './pasteAttachments';

type AttachmentKind = 'archive' | 'audio' | 'code' | 'document' | 'font' | 'image' | 'presentation' | 'spreadsheet' | 'video' | 'file';

type ChatGptAttachmentPreviewProps = {
  attachment: ChatGptTextAttachment;
  onRemove: () => void;
};

export function ChatGptAttachmentPreview({ attachment, onRemove }: ChatGptAttachmentPreviewProps) {
  const [imageOpen, setImageOpen] = useState(false);
  const extension = attachmentExtension(attachment.name);
  const imageSrc = attachmentImageDataUrl(attachment);
  const kind = attachmentKind(attachment, extension);
  const title = attachmentTitle(attachment);

  return <>
    <span className={`chatgpt-file-preview ${imageSrc ? 'is-image' : 'is-file'}`} title={title}>
      {imageSrc
        ? <span className="chatgpt-file-thumb"><button type="button" aria-label={tr('Preview image {name}', { name: attachment.name })} onClick={() => setImageOpen(true)}><img src={imageSrc} alt="" /></button></span>
        : <span className={`chatgpt-file-type-icon kind-${kind}`} aria-hidden="true">
            <AttachmentKindIcon kind={kind} />
            <small>{extension || 'FILE'}</small>
          </span>}
      <span className="chatgpt-file-preview-copy">
        <strong>{attachment.name}</strong>
        <small>{attachmentMeta(attachment, extension)}</small>
      </span>
      <button type="button" aria-label={tr('Remove file {name}', { name: attachment.name })} onClick={onRemove}><X /></button>
    </span>
    {imageOpen && imageSrc && createPortal(<Modal className="chatgpt-image-preview-modal" title={attachment.name} close={() => setImageOpen(false)}>
      <div className="chatgpt-image-preview-body"><img src={imageSrc} alt={attachment.name} /></div>
    </Modal>, document.body)}
  </>;
}

export function attachmentImageDataUrl(attachment: ChatGptTextAttachment) {
  if (attachment.encoding !== 'base64' || !attachment.mimeType.toLowerCase().startsWith('image/') || !attachment.content) return undefined;
  return `data:${attachment.mimeType};base64,${attachment.content}`;
}

export function attachmentExtension(name: string) {
  const cleanName = name.trim().split(/[\\/]/).pop() ?? '';
  const dot = cleanName.lastIndexOf('.');
  if (dot <= 0 || dot === cleanName.length - 1) return '';
  return cleanName.slice(dot + 1).toUpperCase().slice(0, 6);
}

function attachmentKind(attachment: ChatGptTextAttachment, extension: string): AttachmentKind {
  const mime = attachment.mimeType.toLowerCase();
  const ext = extension.toLowerCase();
  if (mime.startsWith('image/')) return 'image';
  if (mime.startsWith('audio/')) return 'audio';
  if (mime.startsWith('video/')) return 'video';
  if (mime.includes('zip') || mime.includes('compressed') || archiveExtensions.has(ext)) return 'archive';
  if (mime.includes('spreadsheet') || mime.includes('excel') || spreadsheetExtensions.has(ext)) return 'spreadsheet';
  if (mime.includes('presentation') || mime.includes('powerpoint') || presentationExtensions.has(ext)) return 'presentation';
  if (mime.includes('font') || fontExtensions.has(ext)) return 'font';
  if (codeExtensions.has(ext) || mime.includes('json') || mime.includes('javascript') || mime.includes('xml')) return 'code';
  if (mime.startsWith('text/') || documentExtensions.has(ext)) return 'document';
  return 'file';
}

function AttachmentKindIcon({ kind }: { kind: AttachmentKind }) {
  switch (kind) {
    case 'archive': return <FileArchive />;
    case 'audio': return <FileAudio2 />;
    case 'code': return <FileCode2 />;
    case 'document': return <FileText />;
    case 'font': return <FileType2 />;
    case 'image': return <FileImage />;
    case 'presentation': return <Presentation />;
    case 'spreadsheet': return <FileSpreadsheet />;
    case 'video': return <FileVideo2 />;
    default: return <File />;
  }
}

function attachmentMeta(attachment: ChatGptTextAttachment, extension: string) {
  const parts = [extension || attachment.mimeType.split(';')[0] || 'FILE'];
  if (typeof attachment.sizeBytes === 'number') parts.push(formatBytes(attachment.sizeBytes));
  else if (attachment.encoding !== 'base64') parts.push(`${attachment.content.length.toLocaleString()} chars`);
  return parts.join(' · ');
}

function attachmentTitle(attachment: ChatGptTextAttachment) {
  if (typeof attachment.sizeBytes === 'number') return `${attachment.name} · ${formatBytes(attachment.sizeBytes)}`;
  return `${attachment.name} · ${attachment.content.length.toLocaleString()} characters`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const archiveExtensions = new Set(['7z', 'bz2', 'gz', 'rar', 'tar', 'xz', 'zip']);
const spreadsheetExtensions = new Set(['csv', 'ods', 'xls', 'xlsm', 'xlsx']);
const presentationExtensions = new Set(['odp', 'ppt', 'pptx']);
const fontExtensions = new Set(['eot', 'otf', 'ttf', 'woff', 'woff2']);
const documentExtensions = new Set(['doc', 'docx', 'md', 'odt', 'pdf', 'rtf', 'txt']);
const codeExtensions = new Set([
  'bash', 'c', 'cpp', 'cs', 'css', 'go', 'h', 'hpp', 'html', 'ini', 'java', 'js', 'jsx', 'json', 'kt', 'lua', 'php', 'ps1', 'py', 'rb', 'rs',
  'scss', 'sh', 'sql', 'svelte', 'toml', 'ts', 'tsx', 'vue', 'xml', 'yaml', 'yml',
]);
