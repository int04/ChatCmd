# ChatCmdClient Local Transport Protocol

ChatCmdClient no longer uses custom application-layer encryption for the local management API or the shared WebSocket transport. The previous ECDH/HKDF/AES-GCM handshake, crypto-session registry, encrypted body wrapper, and encrypted binary WebSocket frames were removed to reduce protocol complexity and runtime overhead.

## Local HTTP API

- Management calls use ordinary JSON request and response bodies under `/api/local/...`.
- The web UI identifies itself with `X-ChatCmdClient: local-ui`; GUI authentication/session checks still apply where required.
- The ChatGPT extension continues to identify itself with `X-ChatCmdClient: chatgpt-extension` and remains restricted by the server-side extension route allowlist.
- There is no `/api/local/crypto/handshake`, crypto session header, encrypted packet envelope, or automatic crypto retry/reset path.
- Remote services should use the configured transport security such as HTTPS when confidentiality on the network is required.

## WebSocket

- `/ws` uses ordinary JSON text frames.
- The server publishes `AppEvent` values as JSON text.
- The web UI and extension send small JSON control messages such as `client.ready` and `client.ping`.
- Invalid/non-JSON application frames are rejected by closing the connection; normal WebSocket ping/pong handling remains available.
- Event redaction and payload bounds remain independent of transport encryption and still apply before publication.

## Security boundaries that remain

Removing the custom crypto layer does not remove authentication or authorization. The project still relies on caller markers, GUI authentication, extension allowlists, task/runtime policy, origin checks where applicable, and loopback/local deployment expectations. Application-layer encryption should not be reintroduced as an obfuscation layer; use standard transport security for non-local network exposure.

## Implementation references

- `src/api/routes.rs` - local API routing and caller-marker middleware.
- `src/websocket/mod.rs` - JSON WebSocket event transport.
- `web/src/api.ts` - web UI JSON API client.
- `web/src/realtime.ts` - web UI JSON WebSocket client.
- `chatgpt-extension/approval-bridge.js` - extension approval WebSocket client.
