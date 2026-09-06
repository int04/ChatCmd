export type CompactPhase = 'preparing' | 'writing_handoff' | 'saving_handoff' | 'opening_new_chat' | 'completed' | 'cancelled';

export interface CompactJob {
  id: string;
  taskId: string;
  phase: CompactPhase;
  revision: number;
  continueAfterCompact: boolean;
  oldConversationId: string | null;
  oldConversationUrl: string | null;
  oldModel: string | null;
  oldRequestId: string | null;
  oldScopeHash: string | null;
  newConversationId: string | null;
  newConversationUrl: string | null;
  handoffText?: string | null;
  detail: string | null;
  createdAtMs: number;
  updatedAtMs: number;
  completedAtMs: number | null;
}

export interface CompactHistory { active: CompactJob | null; history: CompactJob[] }

export const compactSteps = [
  { phase: 'preparing', label: 'Preparing' },
  { phase: 'writing_handoff', label: 'Writing the handoff' },
  { phase: 'saving_handoff', label: 'Saving it' },
  { phase: 'opening_new_chat', label: 'Opening the new chat' },
] as const;

export const compactConfirmation = 'Bạn có muốn tóm tắt context để mang sang cuộc trò chuyện mới không? (Sử dụng khi đoạn trò chuyện này bị chặn, hoặc muốn mở cuộc trò chuyện mà không mất nội dung đang trò chuyện)?';
export const isCompactTerminal = (job: CompactJob) => job.phase === 'completed' || job.phase === 'cancelled';
export const newestCompactFirst = (a: CompactJob, b: CompactJob) => b.createdAtMs - a.createdAtMs || b.updatedAtMs - a.updatedAtMs || a.id.localeCompare(b.id);

/** Reference links must never invoke task switching or execute an untrusted URL. */
export function compactReferenceUrl(value: string | null): string | undefined {
  if (!value) return undefined;
  try {
    const url = new URL(value);
    if (url.protocol !== 'https:' || url.username || url.password || url.port) return undefined;
    if (!['chatgpt.com', 'chat.openai.com'].includes(url.hostname)) return undefined;
    return /\/c\/[^/]+\/?$/.test(url.pathname) ? url.href : undefined;
  } catch { return undefined; }
}
