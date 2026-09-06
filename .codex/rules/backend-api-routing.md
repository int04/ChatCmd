# Backend API routing rule

## Mandatory request path

- React/web UI must never call ChatCMD backend/server URLs directly.
- All backend requests from the UI must follow this path:

  `React -> ChatCmdClient local API -> ChatCMD.Api backend`

- Web code must use the local API client/gateway under `/api/local/backend/*` (or a dedicated local endpoint that internally uses the same backend client).
- Do not put a ChatCMD backend base URL, backend host, backend port, or production backend domain in `web/src/**`.
- Do not use `fetch`, Axios, WebSocket, EventSource, or another browser transport to contact ChatCMD.Api directly.

## Local app to backend

- The Rust app owns the backend base URL and backend HTTP client.
- Application-layer JSON encryption is not used. Send ordinary JSON request and response bodies and rely on the configured transport (for example HTTPS for remote services) when transport security is required.
- Do not add custom crypto handshakes, crypto sessions, encrypted JSON wrappers, or browser-managed application-layer keys.
- Forward only explicitly allowed headers through the local gateway. Never blindly forward all browser headers.

## Build routing

- Backend URLs belong only in Rust source, never in React code.
- Debug builds use `DEBUG_BACKEND_API_URL` in `src/backend_api.rs`.
- Release builds use `RELEASE_BACKEND_API_URL` in `src/backend_api.rs`; set this constant to the production ChatCMD.Api URL before publishing.
- Runtime/build environment variables must not override the backend URL in packaged builds.

## Adding new backend features

- Prefer a purpose-specific local API endpoint when the feature needs local state, credential storage, device information, file access, or response transformation.
- The generic `/api/local/backend/*` gateway is acceptable for ordinary JSON APIs and should use the normal Rust HTTP client without a custom application-layer encryption wrapper.
- Binary downloads, multipart uploads, SSE, streaming responses, and WebSocket backend features require an explicit protocol design; do not assume the JSON gateway supports them.
