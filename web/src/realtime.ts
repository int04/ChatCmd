import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { RealtimeState, TimelineEvent } from './types';

type RealtimeListener = (event: TimelineEvent) => void;
type RealtimeContextValue = { state: RealtimeState; subscribe: (listener: RealtimeListener) => () => void };
type ServerHello = { type: 'crypto.serverHello'; protocol: number; publicKey: string; salt: string };

const RealtimeContext = createContext<RealtimeContextValue | null>(null);
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
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export function RealtimeProvider({ children, WebSocketImpl = WebSocket }: { children: ReactNode; WebSocketImpl?: typeof WebSocket }) {
  const [state, setState] = useState<RealtimeState>('offline');
  const [listeners] = useState(() => new Set<RealtimeListener>());

  useEffect(() => {
    let socket: WebSocket | undefined;
    let timer: number | undefined;
    let stopped = false;
    let attempt = 0;
    const seen = new Set<string>();

    const connect = () => {
      if (stopped) return;
      setState(attempt ? 'reconnecting' : 'offline');
      const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const currentSocket = new WebSocketImpl(`${protocol}//${location.host}/ws`);
      socket = currentSocket;
      currentSocket.binaryType = 'arraybuffer';

      let keyPair: CryptoKeyPair | undefined;
      let sessionKey: CryptoKey | undefined;
      let messageChain = Promise.resolve();

      currentSocket.onopen = () => {
        void (async () => {
          try {
            keyPair = await crypto.subtle.generateKey(
              { name: 'ECDH', namedCurve: 'P-256' },
              false,
              ['deriveBits'],
            );
            const publicKey = await crypto.subtle.exportKey('raw', keyPair.publicKey);
            const handshakeKey = await importHandshakeKey();
            const packet = await encryptHandshake(handshakeKey, {
              type: 'crypto.clientHello',
              protocol: WS_CRYPTO_PROTOCOL,
              publicKey: toBase64Url(new Uint8Array(publicKey)),
            });
            currentSocket.send(toArrayBuffer(packet));
          } catch {
            socket?.close();
          }
        })();
      };

      socket.onmessage = ({ data }) => {
        messageChain = messageChain
          .then(async () => {
            if (!sessionKey) {
              if (!keyPair || typeof data === 'string') throw new Error('Invalid WebSocket handshake frame');
              const handshakeKey = await importHandshakeKey();
              const hello = await decryptHandshake(handshakeKey, data);
              if (hello.type !== 'crypto.serverHello' || hello.protocol !== WS_CRYPTO_PROTOCOL) {
                throw new Error('Unsupported WebSocket crypto handshake');
              }
              sessionKey = await deriveSessionKey(keyPair.privateKey, hello);
              await sendEncrypted(currentSocket, sessionKey, { type: 'client.ready' });
              attempt = 0;
              setState('online');
              return;
            }

            if (typeof data === 'string') throw new Error('Plaintext WebSocket frame is forbidden');
            const event = await decryptEvent(sessionKey, data);
            if (!event.id || !event.type || seen.has(event.id)) return;
            seen.add(event.id);
            if (seen.size > 2000) seen.delete(seen.values().next().value!);
            for (const listener of listeners) listener(event);
          })
          .catch(() => socket?.close());
      };

      socket.onerror = () => socket?.close();
      socket.onclose = () => {
        keyPair = undefined;
        sessionKey = undefined;
        if (stopped) return;
        setState('reconnecting');
        const delay = Math.min(30_000, 500 * 2 ** attempt++);
        timer = window.setTimeout(connect, delay);
      };
    };

    connect();
    return () => {
      stopped = true;
      if (timer) window.clearTimeout(timer);
      socket?.close();
    };
  }, [WebSocketImpl, listeners]);

  const subscribe = useCallback((listener: RealtimeListener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }, [listeners]);
  const value = useMemo<RealtimeContextValue>(() => ({ state, subscribe }), [state, subscribe]);

  return createElement(RealtimeContext.Provider, { value }, children);
}

async function importHandshakeKey(): Promise<CryptoKey> {
  const keyBytes = new Uint8Array(32);
  for (let index = 0; index < keyBytes.length; index += 1) {
    keyBytes[index] = WS_HANDSHAKE_KEY_A[index] ^ WS_HANDSHAKE_KEY_B[index];
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

async function encryptHandshake(key: CryptoKey, value: unknown): Promise<Uint8Array> {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = textEncoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: nonce, additionalData: WS_HANDSHAKE_AAD, tagLength: 128 },
    key,
    plaintext,
  ));
  const packet = new Uint8Array(1 + nonce.length + ciphertext.length);
  packet[0] = WS_CRYPTO_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  return packet;
}

async function decryptHandshake(key: CryptoKey, data: ArrayBuffer | Blob): Promise<ServerHello> {
  const packet = await toBytes(data);
  if (packet.length <= 13 || packet[0] !== WS_CRYPTO_PROTOCOL) throw new Error('Invalid handshake frame');
  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: packet.slice(1, 13), additionalData: WS_HANDSHAKE_AAD, tagLength: 128 },
    key,
    packet.slice(13),
  );
  return JSON.parse(textDecoder.decode(plaintext)) as ServerHello;
}

async function deriveSessionKey(privateKey: CryptoKey, hello: ServerHello): Promise<CryptoKey> {
  const serverPublicKey = await crypto.subtle.importKey(
    'raw',
    toArrayBuffer(fromBase64Url(hello.publicKey)),
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    [],
  );
  const sharedSecret = await crypto.subtle.deriveBits(
    { name: 'ECDH', public: serverPublicKey },
    privateKey,
    256,
  );
  const hkdfMaterial = await crypto.subtle.importKey('raw', sharedSecret, 'HKDF', false, ['deriveKey']);
  new Uint8Array(sharedSecret).fill(0);
  return crypto.subtle.deriveKey(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt: toArrayBuffer(fromBase64Url(hello.salt)),
      info: WS_HKDF_INFO,
    },
    hkdfMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  );
}

async function sendEncrypted(socket: WebSocket, key: CryptoKey, value: unknown) {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = textEncoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: nonce, additionalData: WS_AAD, tagLength: 128 },
    key,
    plaintext,
  ));
  const packet = new Uint8Array(1 + nonce.length + ciphertext.length);
  packet[0] = WS_CRYPTO_PROTOCOL;
  packet.set(nonce, 1);
  packet.set(ciphertext, 13);
  socket.send(packet.buffer);
}

async function decryptEvent(key: CryptoKey, data: string | ArrayBuffer | Blob): Promise<TimelineEvent> {
  const packet = await toBytes(data);
  if (packet.length <= 13 || packet[0] !== WS_CRYPTO_PROTOCOL) throw new Error('Invalid encrypted WebSocket frame');
  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: packet.slice(1, 13), additionalData: WS_AAD, tagLength: 128 },
    key,
    packet.slice(13),
  );
  return JSON.parse(textDecoder.decode(plaintext)) as TimelineEvent;
}

async function toBytes(data: string | ArrayBuffer | Blob): Promise<Uint8Array> {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  throw new Error('Plaintext WebSocket payload is not allowed after handshake');
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

export function useRealtime(onEvent: RealtimeListener) {
  const realtime = useContext(RealtimeContext);
  const listenerRef = useRef(onEvent);
  useEffect(() => {
    listenerRef.current = onEvent;
  }, [onEvent]);
  useEffect(() => realtime?.subscribe((event) => listenerRef.current(event)), [realtime]);
  if (!realtime) throw new Error('useRealtime must be used within RealtimeProvider');
  return realtime.state;
}
