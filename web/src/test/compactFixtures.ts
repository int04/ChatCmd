import type { CompactJob } from '../chatgpt/compact/types';
import type { ChatGptBridge, ChatGptRequest, TaskDetail } from '../types';

export const compactTaskId = 'task-chat-compact-test';
export const oldUrl = 'https://chatgpt.com/g/g-p-project/c/old-chat';
export const newUrl = 'https://chatgpt.com/g/g-p-project/c/new-chat';
export function compactJob(patch: Partial<CompactJob> = {}): CompactJob {
  return {
    id: 'compact-test-job', taskId: compactTaskId, phase: 'preparing', revision: 1, continueAfterCompact: false,
    oldConversationId: 'old-chat', oldConversationUrl: oldUrl, oldModel: 'Thinking',
    oldRequestId: 'old-request', oldScopeHash: 'old-scope', newConversationId: null,
    newConversationUrl: null, detail: null, createdAtMs: 1_780_000_000_000,
    updatedAtMs: 1_780_000_000_000, completedAtMs: null, ...patch,
  };
}
export const compactBridge: ChatGptBridge = {
  taskId: compactTaskId, conversationId: 'old-chat', conversationUrl: oldUrl,
  model: 'Thinking', taskStatus: 'completed', activeStatus: 'completed',
};
export const nextRequest: ChatGptRequest = {
  id: 'next-request', taskId: compactTaskId, turnId: 'next-turn', agentId: 'test-agent',
  model: 'Auto', userContent: 'My preserved draft', submittedContent: 'My preserved draft', status: 'running',
};
export const compactTask: TaskDetail = {
  task: { id: compactTaskId, source: 'chatgpt_web', title: 'The same task',
    status: 'completed', updatedAtUtc: '2026-06-01T12:00:00Z', createdAtUtc: '2026-06-01T11:00:00Z' },
  turns: [], events: [], executionMode: 'allowAll',
};
export const extensionReady = { ready: true, chatGptTabOpen: true, conversationTabOpen: true, conversationReady: true };
