import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { ArrowDown, ArrowUp, Check, Clock3, LoaderCircle, Pencil, Trash2, X, Zap } from 'lucide-react';
import { api } from '../api';
import { Modal } from '../components';
import { tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { ChatGptQueuedMessage } from '../types';
import { useLoad } from '../useLoad';
import { LONG_PASTE_TEXT_THRESHOLD } from './pasteAttachments';

export type ChatGptQueueMode = 'queued' | 'immediate';
export const CHATGPT_QUEUE_TEXT_LIMIT = LONG_PASTE_TEXT_THRESHOLD - 1;

export function clampChatGptQueueDraft(value: string) {
  return value.slice(0, CHATGPT_QUEUE_TEXT_LIMIT);
}

export function ChatGptMessageQueuePanel({
  taskId,
  openMode,
  onOpenModeChange,
  canAutoSend,
  paused = false,
  onAutoSend,
  prepareMessage = (content) => content.trim(),
  onMessageCreated,
}: {
  taskId: string;
  openMode: ChatGptQueueMode | null;
  onOpenModeChange: (mode: ChatGptQueueMode | null) => void;
  canAutoSend: boolean;
  paused?: boolean;
  onAutoSend: (content: string) => Promise<boolean>;
  prepareMessage?: (content: string) => string;
  onMessageCreated?: () => void;
}) {
  const queue = useLoad(() => api.chatGptQueue(taskId), [taskId]);
  const queueData = queue.data;
  const refreshQueue = queue.refresh;
  const [draft, setDraft] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingContent, setEditingContent] = useState('');
  const [busyId, setBusyId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [autoSendingId, setAutoSendingId] = useState<string | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    if (openMode) {
      setDraft('');
      setError('');
    }
  }, [openMode]);

  useEffect(() => {
    if (canAutoSend) void refreshQueue();
  }, [canAutoSend, refreshQueue]);

  useRealtime((event) => {
    if (event.taskId !== taskId || !event.type.startsWith('chatgpt.queue.')) return;
    if (event.type === 'chatgpt.queue.consumed') {
      const payload = event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload)
        ? event.payload as Record<string, unknown>
        : undefined;
      const ids = Array.isArray(payload?.messageIds) ? payload.messageIds.filter((id): id is string => typeof id === 'string') : [];
      if (editingId && ids.includes(editingId)) {
        setEditingId(null);
        setEditingContent('');
      }
    }
    void refreshQueue();
  });

  useEffect(() => {
    if (paused || !canAutoSend || autoSendingId || busyId || editingId) return;
    const next = queueData?.[0];
    if (!next || next.mode !== 'queued') return;
    setAutoSendingId(next.id);
    void (async () => {
      try {
        const sent = await onAutoSend(next.content);
        if (sent) await api.deleteChatGptQueuedMessage(taskId, next.id);
      } catch (reason) {
        setError(errorText(reason));
      } finally {
        setAutoSendingId(null);
        await refreshQueue();
      }
    })();
  }, [paused, autoSendingId, busyId, canAutoSend, editingId, onAutoSend, queueData, refreshQueue, taskId]);

  const create = async () => {
    if (draft.length > CHATGPT_QUEUE_TEXT_LIMIT) return;
    const content = prepareMessage(draft);
    if (paused || !openMode || !content || creating) return;
    setCreating(true);
    setError('');
    try {
      await api.createChatGptQueuedMessage(taskId, { content, mode: openMode });
      onOpenModeChange(null);
      setDraft('');
      onMessageCreated?.();
      await queue.refresh();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setCreating(false);
    }
  };

  const update = async (message: ChatGptQueuedMessage, input: { content?: string; mode?: ChatGptQueueMode }) => {
    if (paused || busyId) return;
    setBusyId(message.id);
    setError('');
    try {
      await api.updateChatGptQueuedMessage(taskId, message.id, input);
      if (input.content !== undefined) {
        setEditingId(null);
        setEditingContent('');
      }
      await queue.refresh();
    } catch (reason) {
      setError(errorText(reason));
      await queue.refresh();
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (message: ChatGptQueuedMessage) => {
    if (paused || busyId) return;
    setBusyId(message.id);
    setError('');
    try {
      await api.deleteChatGptQueuedMessage(taskId, message.id);
      if (editingId === message.id) {
        setEditingId(null);
        setEditingContent('');
      }
      await queue.refresh();
    } catch (reason) {
      setError(errorText(reason));
      await queue.refresh();
    } finally {
      setBusyId(null);
    }
  };

  const move = async (index: number, offset: -1 | 1) => {
    const messages = queue.data ?? [];
    const target = index + offset;
    if (target < 0 || target >= messages.length || paused || busyId) return;
    const reordered = [...messages];
    [reordered[index], reordered[target]] = [reordered[target], reordered[index]];
    queue.setData(reordered);
    setBusyId(messages[index].id);
    setError('');
    try {
      await api.reorderChatGptQueue(taskId, reordered.map((message) => message.id));
      await queue.refresh();
    } catch (reason) {
      setError(errorText(reason));
      await queue.reload();
    } finally {
      setBusyId(null);
    }
  };

  const messages = queue.data ?? [];
  return <>
    {(messages.length > 0 || queue.error || error) && <section className="chatgpt-message-queue" aria-label={tr('Queued ChatGPT messages')}>
      {messages.map((message, index) => {
        const pending = paused || busyId === message.id || autoSendingId === message.id;
        const editing = editingId === message.id;
        return <div className={`chatgpt-queue-item ${message.mode}`} key={message.id}>
          <span className="chatgpt-queue-state" title={message.mode === 'immediate' ? tr('Waiting for AI to receive immediately') : tr('Waiting to send')}>
            {pending ? <LoaderCircle className="spin" /> : message.mode === 'immediate' ? <Zap /> : <Clock3 />}
          </span>
          <div className="chatgpt-queue-content">
            {editing
              ? <textarea rows={2} value={editingContent} onChange={(event) => setEditingContent(event.target.value)} autoFocus />
              : <span title={message.content}>{message.content}</span>}
          </div>
          <div className="chatgpt-queue-actions">
            <button type="button" title={tr('Move up')} aria-label={tr('Move up')} disabled={pending || index === 0} onClick={() => void move(index, -1)}><ArrowUp /></button>
            <button type="button" title={tr('Move down')} aria-label={tr('Move down')} disabled={pending || index === messages.length - 1} onClick={() => void move(index, 1)}><ArrowDown /></button>
            <button className={`chatgpt-queue-priority ${message.mode === 'immediate' ? 'active' : ''}`} type="button" disabled={pending || editing} onClick={() => void update(message, { mode: message.mode === 'immediate' ? 'queued' : 'immediate' })}>
              {message.mode === 'immediate' ? tr('Cancel immediate') : tr('Send immediately')}
            </button>
            {editing
              ? <>
                <button type="button" title={tr('Save')} aria-label={tr('Save')} disabled={pending || !editingContent.trim()} onClick={() => void update(message, { content: editingContent.trim() })}><Check /></button>
                <button type="button" title={tr('Cancel')} aria-label={tr('Cancel')} disabled={pending} onClick={() => { setEditingId(null); setEditingContent(''); }}><X /></button>
              </>
              : <button type="button" title={tr('Edit')} aria-label={tr('Edit')} disabled={pending} onClick={() => { setEditingId(message.id); setEditingContent(message.content); }}><Pencil /></button>}
            <button className="danger" type="button" title={tr('Delete')} aria-label={tr('Delete')} disabled={pending} onClick={() => void remove(message)}><Trash2 /></button>
          </div>
        </div>;
      })}
      {(queue.error || error) && <p className="chatgpt-queue-error" role="alert">{error || queue.error}</p>}
    </section>}
    {openMode && createPortal(<Modal
      className="chatgpt-queue-modal"
      title={openMode === 'immediate' ? tr('Send immediate message') : tr('Queue another message')}
      description={openMode === 'immediate'
        ? tr('AI will receive this on its next MCP call in this conversation. If the turn ends first, it becomes a normal queued message.')
        : tr('This message will be sent automatically when this ChatGPT conversation is ready for the next message.')}
      close={() => !creating && onOpenModeChange(null)}
    >
      <textarea rows={5} value={draft} maxLength={CHATGPT_QUEUE_TEXT_LIMIT} onChange={(event) => setDraft(clampChatGptQueueDraft(event.target.value))} autoFocus placeholder={tr('Enter message…')} disabled={creating} />
      <div className="chatgpt-queue-character-count" aria-live="polite">{draft.length.toLocaleString()} / {CHATGPT_QUEUE_TEXT_LIMIT.toLocaleString()}</div>
      <div className="modal-actions">
        <button className="button secondary" type="button" onClick={() => onOpenModeChange(null)} disabled={creating}>{tr('Cancel')}</button>
        <button className="button primary" type="button" onClick={() => void create()} disabled={paused || creating || !draft.trim()}>
          {creating && <LoaderCircle className="spin" />}{openMode === 'immediate' ? tr('Send immediately') : tr('Add to queue')}
        </button>
      </div>
    </Modal>, document.body)}
  </>;
}

function errorText(reason: unknown) {
  return reason instanceof Error ? reason.message : tr('Could not update queued ChatGPT messages.');
}
