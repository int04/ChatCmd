import { FileText, FolderOpen, Paperclip } from 'lucide-react';
import { useRef, useState, type ClipboardEvent, type ClipboardEventHandler, type Dispatch, type DragEvent, type ReactNode, type SetStateAction } from 'react';
import { createPortal } from 'react-dom';

import { api } from '../api';
import { Modal } from '../components';
import { useAppLanguage } from '../i18n';
import { clipboardAttachmentFromFile, fileAttachmentFromFile, type ChatGptTextAttachment } from './pasteAttachments';

type DroppedFile = { file: File; path?: string };
type DropEntry = { isDirectory?: boolean; isFile?: boolean; name?: string };

type ComposerFileInputProps = {
  value: string;
  setValue: Dispatch<SetStateAction<string>>;
  attachments: ChatGptTextAttachment[];
  setAttachments: Dispatch<SetStateAction<ChatGptTextAttachment[]>>;
  onPaste: ClipboardEventHandler<HTMLTextAreaElement>;
  onError: (message: string) => void;
  disabled?: boolean;
  placeholder: string;
  ariaLabel?: string;
  rows: number;
  variant: 'new' | 'task';
  endAction?: ReactNode;
};

export function ComposerFileInput(props: ComposerFileInputProps) {
  const language = useAppLanguage();
  const copy = language === 'vi' ? viCopy : enCopy;
  const fileInput = useRef<HTMLInputElement>(null);
  const fileSequence = useRef(0);
  const dragDepth = useRef(0);
  const [dragging, setDragging] = useState(false);
  const [reading, setReading] = useState(false);
  const [pendingFiles, setPendingFiles] = useState<DroppedFile[]>([]);

  const attachFiles = async (files: File[], source: 'selected' | 'clipboard' = 'selected') => {
    if (!files.length || reading || props.disabled) return;
    setReading(true);
    try {
      const attachments = await Promise.all(files.map((file) => {
        fileSequence.current += 1;
        return source === 'clipboard'
          ? clipboardAttachmentFromFile(file, fileSequence.current)
          : fileAttachmentFromFile(file, fileSequence.current);
      }));
      props.setAttachments((current) => [...current, ...attachments]);
    } catch {
      props.onError(copy.readError);
    } finally {
      setReading(false);
    }
  };

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = clipboardFiles(event.clipboardData);
    if (!files.length) {
      props.onPaste(event);
      return;
    }
    event.preventDefault();
    void attachFiles(files, 'clipboard');
  };

  const resolveFolderPaths = async (folders: Array<{ name: string; path?: string }>) => {
    const resolved: string[] = [];
    for (const folder of folders) {
      if (folder.path) {
        resolved.push(folder.path);
        continue;
      }
      try {
        const result = await api.pickProjectFolder();
        if (result.path) resolved.push(result.path);
      } catch {
        props.onError(copy.pathError);
      }
    }
    if (resolved.length) props.setValue((current) => appendPaths(current, resolved));
  };

  const handleDrop = async (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    dragDepth.current = 0;
    setDragging(false);
    if (props.disabled) return;

    const data = event.dataTransfer;
    const paths = droppedAbsolutePaths(data);
    const itemEntries = Array.from(data.items).map((item) => ({ item, entry: dropEntry(item) }));
    const folders = itemEntries
      .filter(({ entry }) => entry?.isDirectory)
      .map(({ item, entry }) => ({
        name: entry?.name || copy.folder,
        path: pathFromFile(item.getAsFile()) || matchingPath(paths, entry?.name),
      }));
    const folderNames = new Set(folders.map((folder) => folder.name));
    const files = Array.from(data.files)
      .filter((file) => !folderNames.has(file.name))
      .map((file) => ({ file, path: pathFromFile(file) || matchingPath(paths, file.name) }));

    if (folders.length) await resolveFolderPaths(folders);
    if (files.length) setPendingFiles(files);
  };

  const insertPendingPaths = async () => {
    const resolved: string[] = [];
    for (const pending of pendingFiles) {
      if (pending.path) {
        resolved.push(pending.path);
        continue;
      }
      try {
        const result = await api.pickFilePath();
        if (result.path) resolved.push(result.path);
      } catch {
        props.onError(copy.pathError);
      }
    }
    if (resolved.length) props.setValue((current) => appendPaths(current, resolved));
    setPendingFiles([]);
  };

  const wrapClass = props.variant === 'new' ? 'chatgpt-chat-input-wrap' : 'chatgpt-composer-input-wrap';
  return <>
    <div
      className={`${wrapClass}${dragging ? ' is-dragging' : ''}`}
      onDragEnter={(event) => {
        if (!hasFiles(event.dataTransfer) || props.disabled) return;
        event.preventDefault();
        dragDepth.current += 1;
        setDragging(true);
      }}
      onDragOver={(event) => {
        if (!hasFiles(event.dataTransfer) || props.disabled) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = 'copy';
      }}
      onDragLeave={() => {
        dragDepth.current = Math.max(0, dragDepth.current - 1);
        if (!dragDepth.current) setDragging(false);
      }}
      onDrop={(event) => void handleDrop(event)}
    >
      <textarea
        aria-label={props.ariaLabel}
        rows={props.rows}
        value={props.value}
        onChange={(event) => props.setValue(event.target.value)}
        onPaste={handlePaste}
        disabled={props.disabled}
        placeholder={props.placeholder}
      />
      <input
        ref={fileInput}
        className="chatgpt-file-picker-input"
        type="file"
        multiple
        tabIndex={-1}
        aria-hidden="true"
        onChange={(event) => {
          const files = Array.from(event.target.files ?? []);
          event.target.value = '';
          void attachFiles(files);
        }}
      />
      <button
        className="chatgpt-file-attach-button"
        type="button"
        disabled={props.disabled || reading}
        onClick={() => fileInput.current?.click()}
        aria-label={copy.chooseFile}
        title={copy.chooseFile}
      ><Paperclip /></button>
      {props.endAction}
      {dragging && <div className="chatgpt-file-drop-overlay"><FolderOpen /><span>{copy.dropHere}</span></div>}
    </div>
    {pendingFiles.length > 0 && createPortal(<Modal
      className="chatgpt-file-drop-modal"
      title={copy.dropTitle}
      description={copy.dropDescription}
      close={() => setPendingFiles([])}
    >
      <div className="chatgpt-file-drop-list">
        {pendingFiles.map(({ file }, index) => <div key={`${file.name}-${file.size}-${index}`}><FileText /><span><strong>{file.name}</strong><small>{formatBytes(file.size)}</small></span></div>)}
      </div>
      <div className="modal-actions">
        <button className="button secondary" type="button" onClick={() => void insertPendingPaths()}><FolderOpen />{copy.attachPath}</button>
        <button className="button primary" type="button" onClick={() => { const files = pendingFiles.map((item) => item.file); setPendingFiles([]); void attachFiles(files); }}><Paperclip />{copy.attachFile}</button>
      </div>
    </Modal>, document.body)}
  </>;
}

function dropEntry(item: DataTransferItem): DropEntry | undefined {
  const candidate = item as DataTransferItem & { webkitGetAsEntry?: () => DropEntry | null };
  return candidate.webkitGetAsEntry?.() ?? undefined;
}

function hasFiles(data: DataTransfer) {
  return Array.from(data.types).includes('Files');
}

function clipboardFiles(data: DataTransfer) {
  const itemFiles = Array.from(data.items)
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file));
  return itemFiles.length ? itemFiles : Array.from(data.files);
}

function droppedAbsolutePaths(data: DataTransfer) {
  const paths = Array.from(data.files).map(pathFromFile).filter((value): value is string => Boolean(value));
  for (const type of ['text/uri-list', 'text/plain']) {
    const raw = data.getData(type);
    if (!raw) continue;
    for (const line of raw.split(/\r?\n/)) {
      const candidate = pathFromDroppedText(line);
      if (candidate) paths.push(candidate);
    }
  }
  return [...new Set(paths)];
}

function pathFromFile(file: File | null) {
  if (!file) return undefined;
  const path = (file as File & { path?: unknown }).path;
  return typeof path === 'string' && isAbsolutePath(path) ? path : undefined;
}

function pathFromDroppedText(raw: string) {
  const value = raw.trim().replace(/^['"]|['"]$/g, '');
  if (!value || value.startsWith('#')) return undefined;
  if (value.toLowerCase().startsWith('file://')) {
    try {
      const decoded = decodeURIComponent(value.replace(/^file:\/\//i, ''));
      const windowsPath = decoded.match(/^\/([a-zA-Z]:\/.*)$/)?.[1];
      if (windowsPath) return windowsPath.replace(/\//g, '\\');
      return isAbsolutePath(decoded) ? decoded : undefined;
    } catch {
      return undefined;
    }
  }
  return isAbsolutePath(value) ? value : undefined;
}

function isAbsolutePath(value: string) {
  return /^[a-zA-Z]:[\\/]/.test(value) || /^\\\\/.test(value) || value.startsWith('/');
}

function matchingPath(paths: string[], name?: string) {
  if (!name) return undefined;
  const normalized = name.toLowerCase();
  return paths.find((path) => path.split(/[\\/]/).filter(Boolean).pop()?.toLowerCase() === normalized);
}

function appendPaths(current: string, paths: string[]) {
  const value = paths.map((path) => path.trim()).filter(Boolean).join('\n');
  if (!value) return current;
  if (!current) return value;
  return `${current}${/\s$/.test(current) ? '' : '\n'}${value}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const enCopy = {
  chooseFile: 'Choose file',
  dropHere: 'Drop file or folder here',
  dropTitle: 'How do you want to use this file?',
  dropDescription: 'Attach the dropped file to ChatGPT, or insert only its local path into the message.',
  attachFile: 'Attach file',
  attachPath: 'Attach path',
  readError: 'Could not read the selected or pasted file.',
  pathError: 'Could not resolve the local path.',
  folder: 'Folder',
};
const viCopy = {
  chooseFile: 'Chọn tệp',
  dropHere: 'Thả tệp hoặc thư mục vào đây',
  dropTitle: 'Bạn muốn sử dụng tệp này như thế nào?',
  dropDescription: 'Đính kèm tệp vào ChatGPT hoặc chỉ chèn đường dẫn cục bộ của tệp vào nội dung.',
  attachFile: 'Đính kèm tệp',
  attachPath: 'Đính kèm path',
  readError: 'Không thể đọc tệp đã chọn hoặc dán từ clipboard.',
  pathError: 'Không thể lấy đường dẫn cục bộ.',
  folder: 'Thư mục',
};
