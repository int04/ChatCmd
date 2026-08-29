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
- App-to-backend JSON request and response bodies must use the encrypted backend API protocol. Do not add plaintext app-to-backend JSON calls as a shortcut.
- The browser must not receive or manage the app-to-backend crypto session or backend crypto keys.
- Forward only explicitly allowed headers through the local gateway. Never blindly forward all browser headers.

## Environment routing

- Debug builds default to the local ChatCMD.Api endpoint (`http://127.0.0.1:5121`).
- `CHATCMD_BACKEND_API_URL` may override the backend URL at runtime for local/staging tests.
- Release builds must be built with `CHATCMD_BACKEND_API_RELEASE_URL` set to the production backend URL unless a runtime override is intentionally supplied.
- Do not hardcode production backend domains in React code.

## Adding new backend features

- Prefer a purpose-specific local API endpoint when the feature needs local state, credential storage, device information, file access, or response transformation.
- The generic `/api/local/backend/*` gateway is acceptable for ordinary JSON APIs, but it must still use the Rust encrypted backend client.
- Binary downloads, multipart uploads, SSE, streaming responses, and WebSocket backend features require an explicit protocol design; do not assume the JSON gateway supports them.
