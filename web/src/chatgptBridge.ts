const REQUEST_TYPE = 'chatcmd-chatgpt-extension-request';
const RESPONSE_TYPE = 'chatcmd-chatgpt-extension-response';

type BridgeCommand =
  | { action: 'ping'; nonce: string; conversationUrl?: string }
  | { action: 'send'; nonce: string; requestId: string; submittedContent: string; model: string; conversationUrl?: string; localBaseUrl: string }
  | { action: 'stop'; nonce: string; requestId: string; localBaseUrl: string };

type BridgeResponse = { nonce: string; ok: boolean; error?: string; chatGptTabOpen?: boolean; conversationTabOpen?: boolean; tabId?: number; tabUrl?: string };
export type ChatGptExtensionStatus = { ready: boolean; chatGptTabOpen: boolean; conversationTabOpen: boolean; tabId?: number; tabUrl?: string };

export async function chatGptExtensionStatus(conversationUrl?: string): Promise<ChatGptExtensionStatus> {
  try {
    const response = await bridge({ action: 'ping', nonce: nonce(), conversationUrl }, 1_500);
    return {
      ready: true,
      chatGptTabOpen: response.chatGptTabOpen === true,
      conversationTabOpen: response.conversationTabOpen === true || (!conversationUrl && response.chatGptTabOpen === true),
      tabId: response.tabId,
      tabUrl: response.tabUrl,
    };
  } catch {
    return { ready: false, chatGptTabOpen: false, conversationTabOpen: false };
  }
}

export async function chatGptExtensionAvailable() {
  return (await chatGptExtensionStatus()).ready;
}

export async function dispatchChatGptRequest(input: { requestId: string; submittedContent: string; model: string; conversationUrl?: string }) {
  await bridge({ action: 'send', nonce: nonce(), ...input, localBaseUrl: window.location.origin }, 5_000);
}

export async function stopChatGptRequest(requestId: string) {
  await bridge({ action: 'stop', nonce: nonce(), requestId, localBaseUrl: window.location.origin }, 5_000);
}

function bridge(command: BridgeCommand, timeoutMs: number) {
  return new Promise<BridgeResponse>((resolve, reject) => {
    const timer = window.setTimeout(() => finish(new Error('Không tìm thấy ChatCMD ChatGPT Bridge extension.')), timeoutMs);
    const onMessage = (event: MessageEvent) => {
      if (event.source !== window || !isResponse(event.data) || event.data.nonce !== command.nonce) return;
      finish(event.data.ok ? undefined : new Error(event.data.error || 'Extension không thể thực hiện yêu cầu.'), event.data);
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
