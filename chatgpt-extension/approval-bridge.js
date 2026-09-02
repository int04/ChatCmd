const APPROVAL_BASE_URL_KEY = 'chatcmd-approval-base-url';
const DEFAULT_APPROVAL_BASE_URL = 'http://127.0.0.1:8080';
const APPROVAL_WS_PROTOCOL = 1;
const APPROVAL_WS_AAD = new TextEncoder().encode('chatcmd/ws/v1');
const APPROVAL_WS_HANDSHAKE_AAD = new TextEncoder().encode('chatcmd/ws/handshake-obfuscation/v1');
const APPROVAL_WS_HKDF_INFO = new TextEncoder().encode('chatcmd/ws/aes-256-gcm/v1');
const APPROVAL_WS_HANDSHAKE_KEY_A = new Uint8Array([
  0x9d, 0x23, 0x71, 0xc4, 0x5a, 0xe8, 0x16, 0x3b, 0x42, 0xaf, 0xd1, 0x67, 0x08, 0xbe, 0x95, 0xf2,
  0x31, 0x6c, 0xa9, 0x0d, 0x77, 0xd4, 0x58, 0x83, 0xe1, 0x4f, 0xb6, 0x2a, 0xc8, 0x19, 0x65, 0x90,
]);
const APPROVAL_WS_HANDSHAKE_KEY_B = new Uint8Array([
  0x4a, 0x91, 0xc6, 0x3e, 0xeb, 0x52, 0xa7, 0xd0, 0xf5, 0x1b, 0x64, 0x92, 0xbd, 0x07, 0x2c, 0x49,
  0xe8, 0xd3, 0x15, 0xba, 0x20, 0x6f, 0xc1, 0x34, 0x97, 0xaa, 0x03, 0xfd, 0x5e, 0xb2, 0x48, 0x27,
]);
const approvalTextEncoder = new TextEncoder();
const approvalTextDecoder = new TextDecoder();
const approvalItems = new Map();
let approvalBaseUrl = DEFAULT_APPROVAL_BASE_URL;
let approvalSocket = null;
let approvalReconnectTimer = null;
let approvalReconnectAttempt = 0;
let approvalConnectionGeneration = 0;

async function startApprovalBridge() {
  const stored = await chrome.storage.local.get(APPROVAL_BASE_URL_KEY);
  const configured = stored[APPROVAL_BASE_URL_KEY];
  if (configured) {
    try { approvalBaseUrl = localOrigin(configured); } catch { /* use default */ }
  }
  connectApprovalSocket();
}

async function configureApprovalBridge(value) {
  if (!value) return;
  const next = localOrigin(value);
  if (next === approvalBaseUrl) return;
  approvalBaseUrl = next;
  await chrome.storage.local.set({ [APPROVAL_BASE_URL_KEY]: next });
  approvalConnectionGeneration += 1;
  if (approvalReconnectTimer) clearTimeout(approvalReconnectTimer);
  approvalReconnectTimer = null;
  approvalSocket?.close();
  approvalSocket = null;
  approvalReconnectAttempt = 0;
  connectApprovalSocket();
}

function connectApprovalSocket() {
  const generation = approvalConnectionGeneration;
  let socket;
  try {
    const url = new URL(approvalBaseUrl);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.pathname = '/ws';
    url.search = '';
    url.hash = '';
    socket = new WebSocket(url.href);
  } catch {
    scheduleApprovalReconnect(generation);
    return;
  }
  approvalSocket = socket;
  socket.binaryType = 'arraybuffer';
  let keyPair;
  let sessionKey;
  let heartbeatTimer = null;
  let messageChain = Promise.resolve();

  socket.onopen = () => {
    void (async () => {
      try {
        keyPair = await crypto.subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, false, ['deriveBits']);
        const publicKey = await crypto.subtle.exportKey('raw', keyPair.publicKey);
        const handshakeKey = await approvalHandshakeKey();
        const packet = await encryptApprovalHandshake(handshakeKey, {
          type: 'crypto.clientHello',
          protocol: APPROVAL_WS_PROTOCOL,
          publicKey: approvalToBase64Url(new Uint8Array(publicKey)),
        });
        socket.send(approvalArrayBuffer(packet));
      } catch {
        socket.close();
      }
    })();
  };

  socket.onmessage = ({ data }) => {
    messageChain = messageChain.then(async () => {
      if (!sessionKey) {
        if (!keyPair || typeof data === 'string') throw new Error('Invalid approval WebSocket handshake frame');
        const handshakeKey = await approvalHandshakeKey();
        const hello = await decryptApprovalHandshake(handshakeKey, data);
        if (hello.type !== 'crypto.serverHello' || hello.protocol !== APPROVAL_WS_PROTOCOL) throw new Error('Unsupported approval WebSocket protocol');
        sessionKey = await deriveApprovalSessionKey(keyPair.privateKey, hello);
        await sendApprovalEncrypted(socket, sessionKey, { type: 'client.ready', client: 'chatgpt-extension' });
        if (heartbeatTimer) clearInterval(heartbeatTimer);
        heartbeatTimer = setInterval(() => {
          if (!sessionKey || socket.readyState !== WebSocket.OPEN) return;
          void sendApprovalEncrypted(socket, sessionKey, { type: 'client.ping' }).catch(() => socket.close());
        }, 20_000);
        approvalReconnectAttempt = 0;
        await resyncApprovalQueue();
        return;
      }
      if (typeof data === 'string') throw new Error('Plaintext approval WebSocket frame is forbidden');
      const event = await decryptApprovalEvent(sessionKey, data);
      await handleApprovalEvent(event);
    }).catch(() => socket.close());
  };
  socket.onerror = () => socket.close();
  socket.onclose = () => {
    if (heartbeatTimer) clearInterval(heartbeatTimer);
    heartbeatTimer = null;
    if (approvalSocket === socket) approvalSocket = null;
    if (generation === approvalConnectionGeneration) scheduleApprovalReconnect(generation);
  };
}

function scheduleApprovalReconnect(generation) {
  if (generation !== approvalConnectionGeneration || approvalReconnectTimer) return;
  const delayMs = Math.min(30_000, 500 * (2 ** approvalReconnectAttempt++));
  approvalReconnectTimer = setTimeout(() => {
    approvalReconnectTimer = null;
    if (generation === approvalConnectionGeneration) connectApprovalSocket();
  }, delayMs);
}

async function handleApprovalEvent(event) {
  if (!event || typeof event !== 'object') return;
  if (event.type === 'system.resync_required' || event.type === 'system.connected') {
    await resyncApprovalQueue();
    return;
  }
  if (event.type === 'conversation.approval_pending' || event.type === 'approval.pending') {
    await resyncApprovalQueue();
    return;
  }
  if (event.type === 'conversation.approval_resolved' && event.taskId) {
    approvalItems.delete(conversationApprovalKey(event.taskId));
    await broadcastApprovalState();
    return;
  }
  if (event.type === 'approval.resolved' && event.taskId) {
    const activityId = event.payload?.activityId;
    if (activityId) approvalItems.delete(activityApprovalKey(event.taskId, activityId));
    await broadcastApprovalState();
  }
}

async function resyncApprovalQueue() {
  try {
    const [conversations, activities] = await Promise.all([
      getJson(approvalBaseUrl, '/api/local/tasks/approvals/pending'),
      getJson(approvalBaseUrl, '/api/local/tasks/activity-approvals/pending'),
    ]);
    const next = new Map();
    for (const task of Array.isArray(conversations) ? conversations : []) {
      if (!task?.id || task.allowExecute !== null) continue;
      const item = {
        key: conversationApprovalKey(task.id),
        kind: 'conversation',
        taskId: task.id,
        title: task.title || task.id,
        deadlineUtc: task.approvalDeadlineUtc || null,
        createdAtUtc: task.createdAtUtc || null,
      };
      next.set(item.key, item);
    }
    for (const approval of Array.isArray(activities) ? activities : []) {
      if (!approval?.taskId || !approval?.activityId) continue;
      const item = {
        key: activityApprovalKey(approval.taskId, approval.activityId),
        kind: 'activity',
        taskId: approval.taskId,
        activityId: approval.activityId,
        turnId: approval.turnId || undefined,
        tool: approval.tool || 'tool',
        input: approval.input ?? null,
        deadlineUtc: approval.approvalDeadlineUtc || null,
        createdAtUtc: approval.createdAtUtc || null,
      };
      next.set(item.key, item);
    }
    approvalItems.clear();
    for (const [key, item] of next) approvalItems.set(key, item);
    await broadcastApprovalState();
  } catch {
    // The local app may be stopped, not authenticated yet, or reconnecting.
  }
}

async function approvalBridgeState() {
  return { items: sortedApprovalItems(), baseUrl: approvalBaseUrl, connected: Boolean(approvalSocket && approvalSocket.readyState === WebSocket.OPEN) };
}

async function resolveGlobalApproval(message) {
  const item = message?.item;
  const decision = message?.decision;
  if (!item?.taskId || !item?.kind) throw new Error('Yêu cầu phê duyệt không hợp lệ.');
  if (item.kind === 'conversation') {
    if (!['allow', 'reject'].includes(decision)) throw new Error('Quyết định phê duyệt đoạn trò chuyện không hợp lệ.');
    await postJson(approvalBaseUrl, `/api/local/tasks/${encodeURIComponent(item.taskId)}/${decision === 'allow' ? 'approve-execution' : 'reject-execution'}`, {});
    approvalItems.delete(conversationApprovalKey(item.taskId));
  } else if (item.kind === 'activity') {
    if (!item.activityId || !['allow', 'allowSimilar', 'reject'].includes(decision)) throw new Error('Quyết định phê duyệt lệnh không hợp lệ.');
    try {
      await postJson(approvalBaseUrl, `/api/local/tasks/${encodeURIComponent(item.taskId)}/activities/${encodeURIComponent(item.activityId)}/approval`, {
        turnId: item.turnId || undefined,
        decision,
        reason: typeof message.reason === 'string' && message.reason.trim() ? message.reason.trim() : undefined,
      });
    } catch (error) {
      if (!String(errorMessage(error)).toLowerCase().includes('no longer pending') && !String(errorMessage(error)).toLowerCase().includes('resolved')) throw error;
    }
    approvalItems.delete(activityApprovalKey(item.taskId, item.activityId));
  } else {
    throw new Error('Loại phê duyệt không được hỗ trợ.');
  }
  await broadcastApprovalState();
  return { resolved: true };
}

async function broadcastApprovalState() {
  const payload = { type: 'chatcmd-global-approval-state', items: sortedApprovalItems() };
  const tabs = await chatGptTabs();
  await Promise.all(tabs.filter((tab) => tab.id).map(async (tab) => {
    try { await sendToChatGpt(tab.id, payload, { quiet: true }); } catch { /* tab can still be loading */ }
  }));
}

function sortedApprovalItems() {
  return [...approvalItems.values()].sort((left, right) => {
    const leftTime = Date.parse(left.createdAtUtc || '') || 0;
    const rightTime = Date.parse(right.createdAtUtc || '') || 0;
    return leftTime - rightTime || left.key.localeCompare(right.key);
  });
}

function conversationApprovalKey(taskId) { return `conversation:${taskId}`; }
function activityApprovalKey(taskId, activityId) { return `activity:${taskId}:${activityId}`; }

async function approvalHandshakeKey() {
  const keyBytes = new Uint8Array(32);
  for (let index = 0; index < keyBytes.length; index += 1) keyBytes[index] = APPROVAL_WS_HANDSHAKE_KEY_A[index] ^ APPROVAL_WS_HANDSHAKE_KEY_B[index];
  const key = await crypto.subtle.importKey('raw', approvalArrayBuffer(keyBytes), { name: 'AES-GCM' }, false, ['encrypt', 'decrypt']);
  keyBytes.fill(0);
  return key;
}

async function encryptApprovalHandshake(key, value) {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = approvalTextEncoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: APPROVAL_WS_HANDSHAKE_AAD, tagLength: 128 }, key, plaintext));
  const packet = new Uint8Array(1 + nonce.length + ciphertext.length);
  packet[0] = APPROVAL_WS_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  return packet;
}

async function decryptApprovalHandshake(key, data) {
  const packet = await approvalBytes(data);
  if (packet.length <= 13 || packet[0] !== APPROVAL_WS_PROTOCOL) throw new Error('Invalid approval handshake frame');
  const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: packet.slice(1, 13), additionalData: APPROVAL_WS_HANDSHAKE_AAD, tagLength: 128 }, key, packet.slice(13));
  return JSON.parse(approvalTextDecoder.decode(plaintext));
}

async function deriveApprovalSessionKey(privateKey, hello) {
  const publicKey = await crypto.subtle.importKey('raw', approvalArrayBuffer(approvalFromBase64Url(hello.publicKey)), { name: 'ECDH', namedCurve: 'P-256' }, false, []);
  const sharedSecret = await crypto.subtle.deriveBits({ name: 'ECDH', public: publicKey }, privateKey, 256);
  const material = await crypto.subtle.importKey('raw', sharedSecret, 'HKDF', false, ['deriveKey']);
  new Uint8Array(sharedSecret).fill(0);
  return crypto.subtle.deriveKey({ name: 'HKDF', hash: 'SHA-256', salt: approvalArrayBuffer(approvalFromBase64Url(hello.salt)), info: APPROVAL_WS_HKDF_INFO }, material, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
}

async function sendApprovalEncrypted(socket, key, value) {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = approvalTextEncoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: APPROVAL_WS_AAD, tagLength: 128 }, key, plaintext));
  const packet = new Uint8Array(1 + nonce.length + ciphertext.length);
  packet[0] = APPROVAL_WS_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  socket.send(packet.buffer);
}

async function decryptApprovalEvent(key, data) {
  const packet = await approvalBytes(data);
  if (packet.length <= 13 || packet[0] !== APPROVAL_WS_PROTOCOL) throw new Error('Invalid encrypted approval frame');
  const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: packet.slice(1, 13), additionalData: APPROVAL_WS_AAD, tagLength: 128 }, key, packet.slice(13));
  return JSON.parse(approvalTextDecoder.decode(plaintext));
}

async function approvalBytes(data) {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  throw new Error('Invalid approval WebSocket payload');
}
function approvalToBase64Url(bytes) { let binary = ''; for (const byte of bytes) binary += String.fromCharCode(byte); return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, ''); }
function approvalFromBase64Url(value) { const base64 = value.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '='); const binary = atob(base64); return Uint8Array.from(binary, (char) => char.charCodeAt(0)); }
function approvalArrayBuffer(bytes) { const copy = new Uint8Array(bytes.byteLength); copy.set(bytes); return copy.buffer; }

void startApprovalBridge();
