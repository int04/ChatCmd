import type { ChatGptFileAttachmentPayload } from './chatgpt/pasteAttachments';
import { tr } from './i18n';
import { ChatGptBridgeTimeoutError } from './chatgpt/bridgeErrors';

const REQUEST_TYPE = 'chatcmd-chatgpt-extension-request';
const RESPONSE_TYPE = 'chatcmd-chatgpt-extension-response';

export const REQUIRED_CHATGPT_EXTENSION_VERSION = '0.1.17';


type BridgeCommand =
  | { action: 'compact-resume'; nonce: string; jobId: string; taskId: string; localBaseUrl: string }
  | { action: 'ping'; nonce: string; conversationUrl?: string; approvalSoundEnabled: boolean }
  | { action: 'prepare-tab'; nonce: string; newConversationUrl?: string }
  | { action: 'open-tab'; nonce: string; conversationUrl: string }
  | { action: 'focus-tab'; nonce: string; conversationUrl: string }
  | { action: 'close-tab'; nonce: string; conversationUrl: string }
  | { action: 'logs'; nonce: string }
  | { action: 'clear-logs'; nonce: string }
  | { action: 'send'; nonce: string; requestId: string; submittedContent: string; model: string; conversationUrl?: string; newConversationUrl?: string; attachments?: ChatGptFileAttachmentPayload[]; localBaseUrl: string }
  | { action: 'subagent-send'; nonce: string; subagentId: string; childTaskId: string; submittedContent: string; attempt: number; model: string; conversationUrl?: string; newConversationUrl?: string; localBaseUrl: string }
  | { action: 'subagent-close'; nonce: string; subagentId: string }
  | { action: 'stop'; nonce: string; requestId: string; localBaseUrl: string }
  | { action: 'reconcile'; nonce: string; requestId: string }
  | { action: 'recover-identity'; nonce: string; requestId: string; submittedContent: string; localBaseUrl: string };

export type ChatGptExtensionLog = { at: string; level: 'info' | 'warn' | 'error' | string; source: string; message: string };
type BridgeResponse = { nonce: string; ok: boolean; recovered?: boolean; reason?: string; error?: string; model?: string; logs?: ChatGptExtensionLog[]; extensionVersion?: string; chatGptTabOpen?: boolean; conversationTabOpen?: boolean; conversationReady?: boolean; tabId?: number; tabUrl?: string };
export type ChatGptExtensionStatus = { ready: boolean; extensionVersion?: string; chatGptTabOpen: boolean; conversationTabOpen: boolean; conversationReady: boolean; tabId?: number; tabUrl?: string };

export async function chatGptExtensionStatus(conversationUrl?: string): Promise<ChatGptExtensionStatus> {
  try {
    const response = await bridge({ action: 'ping', nonce: nonce(), conversationUrl, approvalSoundEnabled: approvalSoundPreference() }, 1_500);
    return {
      ready: true,
      extensionVersion: response.extensionVersion,
      chatGptTabOpen: response.chatGptTabOpen === true,
      conversationTabOpen: response.conversationTabOpen === true || (!conversationUrl && response.chatGptTabOpen === true),
      conversationReady: conversationUrl ? response.conversationReady === true : true,
      tabId: response.tabId,
      tabUrl: response.tabUrl,
    };
  } catch {
    return { ready: false, chatGptTabOpen: false, conversationTabOpen: false, conversationReady: false };
  }
}

export async function chatGptExtensionAvailable() {
  return (await chatGptExtensionStatus()).ready;
}

export async function prepareChatGptModelTab(newConversationUrl?: string) {
  return bridge({ action: 'prepare-tab', nonce: nonce(), newConversationUrl }, 3_000);
}

export async function openChatGptConversationTab(conversationUrl: string) {
  await bridge({ action: 'open-tab', nonce: nonce(), conversationUrl }, 3_000);
}

export async function focusChatGptConversationTab(conversationUrl: string) {
  await bridge({ action: 'focus-tab', nonce: nonce(), conversationUrl }, 3_000);
}

export async function closeChatGptConversationTab(conversationUrl: string) {
  await bridge({ action: 'close-tab', nonce: nonce(), conversationUrl }, 3_000);
}

export async function getChatGptExtensionLogs() {
  const response = await bridge({ action: 'logs', nonce: nonce() }, 2_000);
  return Array.isArray(response.logs) ? response.logs : [];
}

export async function clearChatGptExtensionLogs() {
  await bridge({ action: 'clear-logs', nonce: nonce() }, 2_000);
}

export async function dispatchChatGptRequest(input: { requestId: string; submittedContent: string; model: string; conversationUrl?: string; newConversationUrl?: string; attachments?: ChatGptFileAttachmentPayload[] }) {
  await bridge({ action: 'send', nonce: nonce(), ...input, localBaseUrl: window.location.origin }, 5_000);
}

export async function dispatchSubagentFallback(input: { subagentId: string; childTaskId: string; submittedContent: string; attempt: number; model?: string; conversationUrl?: string; newConversationUrl?: string }) {
  await bridge({ action: 'subagent-send', nonce: nonce(), ...input, model: input.model || 'Auto', localBaseUrl: window.location.origin }, 5_000);
}

export async function closeSubagentFallbackTab(subagentId: string) {
  await bridge({ action: 'subagent-close', nonce: nonce(), subagentId }, 3_000);
}

export async function stopChatGptRequest(requestId: string) {
  await bridge({ action: 'stop', nonce: nonce(), requestId, localBaseUrl: window.location.origin }, 5_000);
}

export async function reconcileChatGptRequest(requestId: string) {
  await bridge({ action: 'reconcile', nonce: nonce(), requestId }, 3_000);
}

export async function recoverChatGptIdentity(requestId: string, submittedContent: string) {
  return bridge({ action: 'recover-identity', nonce: nonce(), requestId, submittedContent, localBaseUrl: window.location.origin }, 5_000);
}

export async function resumeChatGptCompact(jobId: string, taskId: string) {
  await bridge({ action: 'compact-resume', nonce: nonce(), jobId, taskId, localBaseUrl: window.location.origin }, 5_000);
}

function bridge(command: BridgeCommand, timeoutMs: number) {
  return new Promise<BridgeResponse>((resolve, reject) => {
    const timer = window.setTimeout(() => finish(new ChatGptBridgeTimeoutError(tr('ChatCMD ChatGPT Bridge did not respond in time.'))), timeoutMs);
    const onMessage = (event: MessageEvent) => {
      if (event.source !== window || !isResponse(event.data) || event.data.nonce !== command.nonce) return;
      finish(event.data.ok ? undefined : new Error(event.data.error || tr('The extension could not complete the request.')), event.data);
    };
    const finish = (error?: Error, response?: BridgeResponse) => {
      window.clearTimeout(timer);
      window.removeEventListener('message', onMessage);
      if (error) reject(error); else resolve(response ?? { nonce: command.nonce, ok: true });
    };
    window.addEventListener('message', onMessage);
    window.postMessage({ type: REQUEST_TYPE, ...command }, window.location.origin);
  });
}

function isResponse(value: unknown): value is BridgeResponse & { type: string } {
  if (!value || typeof value !== 'object') return false;
  const record = value as Record<string, unknown>;
  return record.type === RESPONSE_TYPE && typeof record.nonce === 'string' && typeof record.ok === 'boolean';
}

function approvalSoundPreference() {
  try {
    const value = JSON.parse(localStorage.getItem('chatcmd.preferences') ?? '{}') as Record<string, unknown>;
    if (typeof value.newAgentSound === 'boolean') return value.newAgentSound;
    return value.sound !== false;
  } catch {
    return true;
  }
}

function nonce() { return crypto.randomUUID(); }
