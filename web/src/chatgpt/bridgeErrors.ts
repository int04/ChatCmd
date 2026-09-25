/** A missing transport ACK leaves dispatch outcome unknown, not failed. */
export class ChatGptBridgeTimeoutError extends Error {
  readonly code = 'bridge_ack_timeout';

  constructor(message: string) {
    super(message);
    this.name = 'ChatGptBridgeTimeoutError';
  }
}
