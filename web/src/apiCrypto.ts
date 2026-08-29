const API_CRYPTO_PROTOCOL = 1;
const API_HANDSHAKE_AAD = new TextEncoder().encode('chatcmd/api/handshake-obfuscation/v1');
const API_HKDF_INFO = new TextEncoder().encode('chatcmd/api/aes-256-gcm/v1');
const API_HANDSHAKE_KEY_A = new Uint8Array([
  0x9d, 0x23, 0x71, 0xc4, 0x5a, 0xe8, 0x16, 0x3b, 0x42, 0xaf, 0xd1, 0x67, 0x08, 0xbe, 0x95, 0xf2,
  0x31, 0x6c, 0xa9, 0x0d, 0x77, 0xd4, 0x58, 0x83, 0xe1, 0x4f, 0xb6, 0x2a, 0xc8, 0x19, 0x65, 0x90,
]);
const API_HANDSHAKE_KEY_B = new Uint8Array([
  0x4a, 0x91, 0xc6, 0x3e, 0xeb, 0x52, 0xa7, 0xd0, 0xf5, 0x1b, 0x64, 0x92, 0xbd, 0x07, 0x2c, 0x49,
  0xe8, 0xd3, 0x15, 0xba, 0x20, 0x6f, 0xc1, 0x34, 0x97, 0xaa, 0x03, 0xfd, 0x5e, 0xb2, 0x48, 0x27,
]);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

type ApiCryptoSession = { id: string; key: CryptoKey };
type ServerHello = { type: 'crypto.serverHello'; protocol: number; sessionId: string; publicKey: string; salt: string };

let sessionPromise: Promise<ApiCryptoSession> | undefined;

export async function encryptedApiFetch(path: string, init: RequestInit): Promise<Response> {
  return encryptedApiFetchAttempt(path, init, false);
}

async function encryptedApiFetchAttempt(path: string, init: RequestInit, retried: boolean): Promise<Response> {
  const session = await apiCryptoSession();
  const method = (init.method ?? 'GET').toUpperCase();
  const headers = new Headers(init.headers);
  headers.set('X-ChatCmdClient', 'local-ui');
  headers.set('X-ChatCmd-Crypto', '1');
  headers.set('X-ChatCmd-Crypto-Session', session.id);

  let body: BodyInit | null | undefined = init.body;
  if (typeof init.body === 'string') {
    body = toArrayBuffer(await encryptPacket(
      session.key,
      encoder.encode(init.body),
      encoder.encode(apiAad('request', method, path)),
    ));
    headers.set('Content-Type', 'application/octet-stream');
  }

  const response = await fetch(path, { ...init, headers, body });
  if (response.headers.get('x-chatcmd-crypto-reset') === '1' && !retried) {
    resetApiCryptoSession();
    return encryptedApiFetchAttempt(path, init, true);
  }
  return response;
}

export async function decodeEncryptedApiResponse<T>(path: string, method: string, response: Response): Promise<T> {
  if (response.headers.get('x-chatcmd-crypto') !== '1') {
    return response.json() as Promise<T>;
  }
  const session = await apiCryptoSession();
  const packet = new Uint8Array(await response.arrayBuffer());
  const plaintext = await decryptPacket(
    session.key,
    packet,
    encoder.encode(apiAad('response', method.toUpperCase(), path, response.status)),
  );
  return JSON.parse(decoder.decode(plaintext)) as T;
}

export function resetApiCryptoSession() {
  sessionPromise = undefined;
}

async function apiCryptoSession(): Promise<ApiCryptoSession> {
  sessionPromise ??= establishApiCryptoSession().catch((error) => {
    sessionPromise = undefined;
    throw error;
  });
  return sessionPromise;
}

async function establishApiCryptoSession(): Promise<ApiCryptoSession> {
  const keyPair = await crypto.subtle.generateKey(
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    ['deriveBits'],
  );
  const publicKey = await crypto.subtle.exportKey('raw', keyPair.publicKey);
  const handshakeKey = await importHandshakeKey();
  const requestPacket = await encryptPacket(
    handshakeKey,
    encoder.encode(JSON.stringify({
      type: 'crypto.clientHello',
      protocol: API_CRYPTO_PROTOCOL,
      publicKey: toBase64Url(new Uint8Array(publicKey)),
    })),
    API_HANDSHAKE_AAD,
  );
  const response = await fetch('/api/local/crypto/handshake', {
    method: 'POST',
    headers: {
      'X-ChatCmdClient': 'local-ui',
      'Content-Type': 'application/octet-stream',
    },
    body: toArrayBuffer(requestPacket),
  });
  if (!response.ok) throw new Error(`API crypto handshake failed (${response.status})`);
  const helloPacket = new Uint8Array(await response.arrayBuffer());
  const hello = JSON.parse(decoder.decode(await decryptPacket(handshakeKey, helloPacket, API_HANDSHAKE_AAD))) as ServerHello;
  if (hello.type !== 'crypto.serverHello' || hello.protocol !== API_CRYPTO_PROTOCOL || !hello.sessionId) {
    throw new Error('Unsupported API crypto handshake');
  }

  const serverPublicKey = await crypto.subtle.importKey(
    'raw',
    toArrayBuffer(fromBase64Url(hello.publicKey)),
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    [],
  );
  const sharedSecret = await crypto.subtle.deriveBits(
    { name: 'ECDH', public: serverPublicKey },
    keyPair.privateKey,
    256,
  );
  const hkdfMaterial = await crypto.subtle.importKey('raw', sharedSecret, 'HKDF', false, ['deriveKey']);
  new Uint8Array(sharedSecret).fill(0);
  const key = await crypto.subtle.deriveKey(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt: toArrayBuffer(fromBase64Url(hello.salt)),
      info: API_HKDF_INFO,
    },
    hkdfMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  );
  return { id: hello.sessionId, key };
}

function apiAad(direction: 'request' | 'response', method: string, path: string, status?: number) {
  return status === undefined
    ? `chatcmd/api/v1|${direction}|${method}|${path}`
    : `chatcmd/api/v1|${direction}|${method}|${path}|${status}`;
}

async function importHandshakeKey(): Promise<CryptoKey> {
  const keyBytes = new Uint8Array(32);
  for (let index = 0; index < keyBytes.length; index += 1) {
    keyBytes[index] = API_HANDSHAKE_KEY_A[index] ^ API_HANDSHAKE_KEY_B[index];
  }
  const key = await crypto.subtle.importKey(
    'raw',
    toArrayBuffer(keyBytes),
    { name: 'AES-GCM' },
    false,
    ['encrypt', 'decrypt'],
  );
  keyBytes.fill(0);
  return key;
}

async function encryptPacket(key: CryptoKey, plaintext: Uint8Array, aad: Uint8Array): Promise<Uint8Array> {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: toArrayBuffer(nonce), additionalData: toArrayBuffer(aad), tagLength: 128 },
    key,
    toArrayBuffer(plaintext),
  ));
  const packet = new Uint8Array(13 + ciphertext.length);
  packet[0] = API_CRYPTO_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  return packet;
}

async function decryptPacket(key: CryptoKey, packet: Uint8Array, aad: Uint8Array): Promise<ArrayBuffer> {
  if (packet.length <= 13 || packet[0] !== API_CRYPTO_PROTOCOL) throw new Error('Invalid encrypted API payload');
  return crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: toArrayBuffer(packet.slice(1, 13)), additionalData: toArrayBuffer(aad), tagLength: 128 },
    key,
    toArrayBuffer(packet.slice(13)),
  );
}

function toBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

function fromBase64Url(value: string): Uint8Array {
  const base64 = value.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '=');
  const binary = atob(base64);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}
