import type { LiveTerminalEvent } from './types';

export function decodeTerminalEvent(event: Pick<LiveTerminalEvent, 'data' | 'encoding'>): string | Uint8Array {
  if (event.encoding !== 'base64') return event.data;
  const binary = window.atob(event.data);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
