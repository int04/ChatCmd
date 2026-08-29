import '@testing-library/jest-dom/vitest';
import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';

const WS_CRYPTO_PROTOCOL = 1;
const WS_AAD = new TextEncoder().encode('chatcmd/ws/v1');
const WS_HKDF_INFO = new TextEncoder().encode('chatcmd/ws/aes-256-gcm/v1');
const encoder = new TextEncoder();

afterEach(() => { cleanup(); vi.useRealTimers(); });
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
    if (typeof data === 'string') {
      const hello = JSON.parse(data) as { type: string; protocol: number; publicKey: string };
      if (hello.type === 'crypto.clientHello' && hello.protocol === WS_CRYPTO_PROTOCOL) {
        this.handshake = this.acceptClientHello(hello.publicKey);
        this.handshakeStartedResolve();
      }
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

  private async acceptClientHello(clientPublicKey: string) {
    const keyPair = await crypto.subtle.generateKey(
      { name: 'ECDH', namedCurve: 'P-256' },
      false,
      ['deriveBits'],
    );
    const clientPublic = await crypto.subtle.importKey(
      'raw',
      toArrayBuffer(fromBase64Url(clientPublicKey)),
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
    this.onmessage?.({
      data: JSON.stringify({
        type: 'crypto.serverHello',
        protocol: WS_CRYPTO_PROTOCOL,
        publicKey: toBase64Url(new Uint8Array(serverPublic)),
        salt: toBase64Url(salt),
      }),
    });
  }
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
