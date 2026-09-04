import '@testing-library/jest-dom/vitest';
import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';

const WS_CRYPTO_PROTOCOL = 1;
const WS_AAD = new TextEncoder().encode('chatcmd/ws/v1');
const WS_HANDSHAKE_AAD = new TextEncoder().encode('chatcmd/ws/handshake-obfuscation/v1');
const WS_HKDF_INFO = new TextEncoder().encode('chatcmd/ws/aes-256-gcm/v1');
const WS_HANDSHAKE_KEY_A = new Uint8Array([
  0x9d, 0x23, 0x71, 0xc4, 0x5a, 0xe8, 0x16, 0x3b, 0x42, 0xaf, 0xd1, 0x67, 0x08, 0xbe, 0x95, 0xf2,
  0x31, 0x6c, 0xa9, 0x0d, 0x77, 0xd4, 0x58, 0x83, 0xe1, 0x4f, 0xb6, 0x2a, 0xc8, 0x19, 0x65, 0x90,
]);
const WS_HANDSHAKE_KEY_B = new Uint8Array([
  0x4a, 0x91, 0xc6, 0x3e, 0xeb, 0x52, 0xa7, 0xd0, 0xf5, 0x1b, 0x64, 0x92, 0xbd, 0x07, 0x2c, 0x49,
  0xe8, 0xd3, 0x15, 0xba, 0x20, 0x6f, 0xc1, 0x34, 0x97, 0xaa, 0x03, 0xfd, 0x5e, 0xb2, 0x48, 0x27,
]);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

const storageState = new Map<string, string>();
const testStorage: Storage = {
  get length() { return storageState.size; },
  clear() { storageState.clear(); },
  getItem(key: string) { return storageState.get(String(key)) ?? null; },
  key(index: number) { return Array.from(storageState.keys())[index] ?? null; },
  removeItem(key: string) { storageState.delete(String(key)); },
  setItem(key: string, value: string) { storageState.set(String(key), String(value)); },
};
Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: testStorage });
Object.defineProperty(window, 'localStorage', { configurable: true, value: testStorage });

afterEach(() => { cleanup(); storageState.clear(); vi.useRealTimers(); });
Object.defineProperty(window, 'matchMedia', { writable: true, value: vi.fn().mockImplementation(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });

export class FakeSocket {
  static instances: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string | ArrayBuffer | Blob }) => void) | null = null;
  binaryType: BinaryType = 'blob';

  private sessionKey?: CryptoKey;
  private handshakeStartedResolve!: () => void;
  private clientReadyResolve!: () => void;
  private readonly handshakeStarted = new Promise<void>((resolve) => { this.handshakeStartedResolve = resolve; });
  private readonly clientReady = new Promise<void>((resolve) => { this.clientReadyResolve = resolve; });
  private handshake?: Promise<void>;

  constructor(public url: string) { FakeSocket.instances.push(this); }

  close() { /* controlled by test */ }
  open() { this.onopen?.(); }
  disconnect() { this.onclose?.(); }

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
    if (typeof data === 'string') throw new Error('plaintext WebSocket frames are not expected');
    if (!this.sessionKey) {
      this.handshake = this.acceptEncryptedClientHello(data);
      this.handshakeStartedResolve();
      return;
    }
    this.clientReadyResolve();
  }

  async ready() {
    await this.handshakeStarted;
    await this.handshake;
    await this.clientReady;
    await Promise.resolve();
  }

  async message(value: unknown) {
    await this.ready();
    if (!this.sessionKey) throw new Error('Fake WebSocket session is not encrypted');
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const plaintext = encoder.encode(JSON.stringify(value));
    const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv: nonce, additionalData: WS_AAD, tagLength: 128 },
      this.sessionKey,
      plaintext,
    ));
    const packet = new Uint8Array(1 + nonce.length + ciphertext.length);
    packet[0] = WS_CRYPTO_PROTOCOL;
    packet.set(nonce, 1);
    packet.set(ciphertext, 13);
    this.onmessage?.({ data: packet.buffer });
    await Promise.resolve();
  }

  private async acceptEncryptedClientHello(data: ArrayBufferLike | Blob | ArrayBufferView) {
    const handshakeKey = await importHandshakeKey();
    const packet = await binaryToBytes(data);
    const hello = await decryptHandshake(handshakeKey, packet) as { type: string; protocol: number; publicKey: string };
    if (hello.type !== 'crypto.clientHello' || hello.protocol !== WS_CRYPTO_PROTOCOL) throw new Error('invalid client hello');

    const keyPair = await crypto.subtle.generateKey(
      { name: 'ECDH', namedCurve: 'P-256' },
      false,
      ['deriveBits'],
    );
    const clientPublic = await crypto.subtle.importKey(
      'raw',
      toArrayBuffer(fromBase64Url(hello.publicKey)),
      { name: 'ECDH', namedCurve: 'P-256' },
      false,
      [],
    );
    const sharedSecret = await crypto.subtle.deriveBits(
      { name: 'ECDH', public: clientPublic },
      keyPair.privateKey,
      256,
    );
    const salt = crypto.getRandomValues(new Uint8Array(32));
    const hkdfMaterial = await crypto.subtle.importKey('raw', sharedSecret, 'HKDF', false, ['deriveKey']);
    new Uint8Array(sharedSecret).fill(0);
    this.sessionKey = await crypto.subtle.deriveKey(
      { name: 'HKDF', hash: 'SHA-256', salt: toArrayBuffer(salt), info: WS_HKDF_INFO },
      hkdfMaterial,
      { name: 'AES-GCM', length: 256 },
      false,
      ['encrypt', 'decrypt'],
    );
    const serverPublic = await crypto.subtle.exportKey('raw', keyPair.publicKey);
    const responsePacket = await encryptHandshake(handshakeKey, {
      type: 'crypto.serverHello',
      protocol: WS_CRYPTO_PROTOCOL,
      publicKey: toBase64Url(new Uint8Array(serverPublic)),
      salt: toBase64Url(salt),
    });
    this.onmessage?.({ data: toArrayBuffer(responsePacket) });
  }
}

async function importHandshakeKey(): Promise<CryptoKey> {
  const keyBytes = new Uint8Array(32);
  for (let index = 0; index < keyBytes.length; index += 1) keyBytes[index] = WS_HANDSHAKE_KEY_A[index] ^ WS_HANDSHAKE_KEY_B[index];
  const key = await crypto.subtle.importKey('raw', toArrayBuffer(keyBytes), { name: 'AES-GCM' }, false, ['encrypt', 'decrypt']);
  keyBytes.fill(0);
  return key;
}

async function encryptHandshake(key: CryptoKey, value: unknown): Promise<Uint8Array> {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = encoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: nonce, additionalData: WS_HANDSHAKE_AAD, tagLength: 128 }, key, plaintext,
  ));
  const packet = new Uint8Array(13 + ciphertext.length);
  packet[0] = WS_CRYPTO_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  return packet;
}

async function decryptHandshake(key: CryptoKey, packet: Uint8Array): Promise<unknown> {
  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: packet.slice(1, 13), additionalData: WS_HANDSHAKE_AAD, tagLength: 128 }, key, packet.slice(13),
  );
  return JSON.parse(decoder.decode(plaintext));
}

async function binaryToBytes(data: ArrayBufferLike | Blob | ArrayBufferView): Promise<Uint8Array> {
  if (data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return new Uint8Array(data);
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

vi.stubGlobal('WebSocket', FakeSocket);
