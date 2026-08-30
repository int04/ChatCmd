import { tr } from './i18n';

const REQUEST_TYPE = 'chatcmd-chatgpt-extension-request';
const RESPONSE_TYPE = 'chatcmd-chatgpt-extension-response';

type BridgeCommand =
  | { action: 'ping'; nonce: string; conversationUrl?: string }
  | { action: 'focus-tab'; nonce: string; conversationUrl: string }
  | { action: 'close-tab'; nonce: string; conversationUrl: string }
  | { action: 'logs'; nonce: string }
  | { action: 'clear-logs'; nonce: string }
  | { action: 'send'; nonce: string; requestId: string; submittedContent: string; model: string; conversationUrl?: string; localBaseUrl: string }
  | { action: 'stop'; nonce: string; requestId: string; localBaseUrl: string };

export type ChatGptExtensionLog = { at: string; level: 'info' | 'warn' | 'error' | string; source: string; message: string };
type BridgeResponse = { nonce: string; ok: boolean; error?: string; model?: string; logs?: ChatGptExtensionLog[]; chatGptTabOpen?: boolean; conversationTabOpen?: boolean; conversationReady?: boolean; tabId?: number; tabUrl?: string };
export type ChatGptExtensionStatus = { ready: boolean; chatGptTabOpen: boolean; conversationTabOpen: boolean; conversationReady: boolean; tabId?: number; tabUrl?: string };

export async function chatGptExtensionStatus(conversationUrl?: string): Promise<ChatGptExtensionStatus> {
  try {
    const response = await bridge({ action: 'ping', nonce: nonce(), conversationUrl }, 1_500);
    return {
      ready: true,
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

export async function dispatchChatGptRequest(input: { requestId: string; submittedContent: string; model: string; conversationUrl?: string }) {
  await bridge({ action: 'send', nonce: nonce(), ...input, localBaseUrl: window.location.origin }, 5_000);
}

export async function stopChatGptRequest(requestId: string) {
  await bridge({ action: 'stop', nonce: nonce(), requestId, localBaseUrl: window.location.origin }, 5_000);
}

function bridge(command: BridgeCommand, timeoutMs: number) {
  return new Promise<BridgeResponse>((resolve, reject) => {
    const timer = window.setTimeout(() => finish(new Error(tr('ChatCMD ChatGPT Bridge did not respond in time.'))), timeoutMs);
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

function nonce() { return crypto.randomUUID(); }
