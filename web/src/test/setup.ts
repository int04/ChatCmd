import '@testing-library/jest-dom/vitest';
import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';

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
  sent: string[] = [];

  constructor(public url: string) { FakeSocket.instances.push(this); }

  close() { /* controlled by test */ }
  open() { this.onopen?.(); }
  disconnect() { this.onclose?.(); }

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
    if (typeof data !== 'string') throw new Error('plaintext WebSocket frames are expected');
    this.sent.push(data);
  }

  async ready() {
    this.open();
    await Promise.resolve();
  }

  async message(value: unknown) {
    this.onmessage?.({ data: JSON.stringify(value) });
    await Promise.resolve();
  }
}

vi.stubGlobal('WebSocket', FakeSocket);
