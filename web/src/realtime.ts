import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { RealtimeState, TimelineEvent } from './types';

type RealtimeListener = (event: TimelineEvent) => void;
type RealtimeContextValue = { state: RealtimeState; subscribe: (listener: RealtimeListener) => () => void };
type ServerHello = { type: 'crypto.serverHello'; protocol: number; publicKey: string; salt: string };

const RealtimeContext = createContext<RealtimeContextValue | null>(null);
const WS_CRYPTO_PROTOCOL = 1;
const WS_AAD = new TextEncoder().encode('chatcmd/ws/v1');
const WS_HKDF_INFO = new TextEncoder().encode('chatcmd/ws/aes-256-gcm/v1');
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
            currentSocket.send(JSON.stringify({
              type: 'crypto.clientHello',
              protocol: WS_CRYPTO_PROTOCOL,
              publicKey: toBase64Url(new Uint8Array(publicKey)),
            }));
          } catch {
            socket?.close();
          }
        })();
      };

      socket.onmessage = ({ data }) => {
        messageChain = messageChain
          .then(async () => {
            if (typeof data === 'string') {
              if (sessionKey || !keyPair) throw new Error('Unexpected plaintext WebSocket frame');
              const hello = JSON.parse(data) as ServerHello;
              if (hello.type !== 'crypto.serverHello' || hello.protocol !== WS_CRYPTO_PROTOCOL) {
                throw new Error('Unsupported WebSocket crypto handshake');
              }
              sessionKey = await deriveSessionKey(keyPair.privateKey, hello);
              await sendEncrypted(currentSocket, sessionKey, { type: 'client.ready' });
              attempt = 0;
              setState('online');
              return;
            }

            if (!sessionKey) throw new Error('Encrypted frame received before handshake');
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
  listenerRef.current = onEvent;
  useEffect(() => realtime?.subscribe((event) => listenerRef.current(event)), [realtime]);
  if (!realtime) throw new Error('useRealtime must be used within RealtimeProvider');
  return realtime.state;
}
