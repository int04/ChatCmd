# ChatCmdClient Encryption Protocol

Tài liệu này mô tả cơ chế mã hóa hiện tại của WebSocket `/ws` và HTTP local API `/api/local/*`.

Mục tiêu của tài liệu là để developer hoặc AI sau này có thể debug nhanh khi gặp lỗi handshake, decrypt thất bại, API trả HTTP 200 nhưng UI vẫn báo lỗi, nhiều tab xung đột session, hoặc dữ liệu đột nhiên xuất hiện plaintext trong DevTools.

> Lưu ý quan trọng: lớp fixed AES-GCM ở bước handshake chỉ dùng để **obfuscate** public handshake data. Bảo mật session thật sự đến từ `ECDH P-256 ephemeral -> HKDF-SHA256 -> AES-256-GCM`.

---

## 1. Tổng quan kiến trúc

Cả WebSocket và HTTP API đều dùng cùng ý tưởng tổng quát:

```text
binary handshake
    -> fixed AES-256-GCM obfuscation
    -> ECDH P-256 ephemeral key agreement
    -> HKDF-SHA256
    -> AES-256-GCM session key
    -> encrypted application payloads
```

Các session được giữ trong RAM, không ghi vào `localStorage`, `sessionStorage`, URL hoặc file cấu hình.

Frontend dùng WebCrypto và tạo private ECDH key với `extractable = false`.

Mỗi tab browser tự tạo handshake/session riêng. Reload tab sẽ tạo key mới. Vì vậy nhiều tab mở đồng thời không được dùng chung session key và không ảnh hưởng lẫn nhau.

---

## 2. WebSocket `/ws`

### 2.1 File liên quan

Backend:

- `src/websocket/mod.rs`

Frontend:

- `web/src/realtime.ts`

Test frontend WebSocket:

- `web/src/test/setup.ts`
- `web/src/test/realtime.test.tsx`
- `web/src/test/App.test.tsx`

---

### 2.2 Constants hiện tại

Protocol version:

```text
1
```

Application AES-GCM AAD:

```text
chatcmd/ws/v1
```

Handshake AES-GCM AAD:

```text
chatcmd/ws/handshake-obfuscation/v1
```

HKDF info:

```text
chatcmd/ws/aes-256-gcm/v1
```

Handshake timeout backend:

```text
10 seconds
```

---

### 2.3 Fixed handshake key

Client và server đều có hai mảng 32 byte:

```text
WS_HANDSHAKE_KEY_A
WS_HANDSHAKE_KEY_B
```

Runtime reconstruct key bằng:

```text
key[i] = KEY_A[i] XOR KEY_B[i]
```

Sau đó import thành AES-256-GCM key.

Mục tiêu của cách chia key là tránh để một chuỗi key 32-byte hoàn chỉnh có thể search thẳng trong source/bundle.

Điều này **không biến fixed key thành secret tuyệt đối**. Người có quyền debug browser vẫn có thể hook WebCrypto hoặc reverse bundle.

---

### 2.4 WebSocket handshake flow

#### Bước 1 - Client tạo ECDH keypair

Frontend gọi WebCrypto:

```text
ECDH
curve = P-256
privateKey extractable = false
```

Client export public key dạng raw SEC1 bytes và Base64URL encode.

Logical payload trước khi obfuscate:

```json
{
  "type": "crypto.clientHello",
  "protocol": 1,
  "publicKey": "..."
}
```

Payload này **không gửi dạng JSON text**.

Client mã hóa JSON trên bằng fixed handshake AES-GCM rồi gửi dưới dạng WebSocket binary frame.

---

#### Bước 2 - Server decrypt `clientHello`

Server nhận frame đầu tiên và yêu cầu frame phải là `Message::Binary`.

Server reconstruct fixed handshake key và decrypt bằng:

```text
AES-256-GCM
AAD = chatcmd/ws/handshake-obfuscation/v1
```

Sau decrypt mới parse `ClientHello` JSON.

Server reject handshake nếu:

- frame đầu không phải binary
- decrypt thất bại
- JSON invalid
- `type != crypto.clientHello`
- `protocol != 1`
- public key P-256 không hợp lệ
- timeout > 10 giây

---

#### Bước 3 - Server tạo ephemeral keypair

Server tạo `EphemeralSecret` P-256 mới cho connection đó.

Sau đó:

```text
sharedSecret = ECDH(serverPrivate, clientPublic)
```

Server sinh random salt 32 byte.

Session key được derive:

```text
HKDF-SHA256(
    input = ECDH shared secret,
    salt = random 32-byte salt,
    info = "chatcmd/ws/aes-256-gcm/v1"
)
```

Output = 32 bytes -> AES-256-GCM session key.

Temporary session key bytes phía Rust được zero sau khi tạo `Aes256Gcm`.

---

#### Bước 4 - Server gửi `serverHello`

Logical payload:

```json
{
  "type": "crypto.serverHello",
  "protocol": 1,
  "publicKey": "...",
  "salt": "..."
}
```

Payload tiếp tục được mã hóa bằng fixed handshake AES-GCM và gửi binary.

Không gửi plaintext JSON.

---

#### Bước 5 - Client derive session key

Client decrypt `serverHello` bằng fixed handshake key.

Sau đó import public key server và tính:

```text
sharedSecret = ECDH(clientPrivate, serverPublic)
```

Client derive AES key bằng cùng HKDF params.

Shared secret ArrayBuffer phía frontend được zero sau khi import vào HKDF material.

Derived AES key là `extractable = false`.

---

#### Bước 6 - Client gửi `client.ready`

Sau khi có session key, client gửi:

```json
{
  "type": "client.ready"
}
```

nhưng payload đã được mã hóa bằng **session AES-GCM**, không còn fixed handshake key.

Sau thời điểm này, mọi application frame phải là encrypted binary.

---

### 2.5 WebSocket encrypted packet format

Cả handshake packet và application packet hiện có layout:

```text
byte 0      : protocol version = 1
byte 1..12  : random AES-GCM nonce, 12 bytes
byte 13..N  : ciphertext + 16-byte GCM authentication tag
```

Mỗi packet phải dùng nonce random mới.

Không reuse nonce với cùng AES key.

---

### 2.6 WebSocket application encryption

Sau handshake:

```text
AES-256-GCM
AAD = chatcmd/ws/v1
```

Server gửi các `AppEvent` dưới dạng encrypted binary frame.

Client nhận binary -> decrypt -> JSON parse -> dispatch realtime listeners.

Server nhận binary client payload -> decrypt -> JSON parse.

Nếu client gửi plaintext `Message::Text` sau handshake, backend đóng connection.

Nếu AES-GCM authentication fail hoặc payload invalid, connection cũng bị kết thúc.

---

### 2.7 Multi-tab và reconnect

Mỗi `RealtimeProvider` instance tạo connection riêng.

Mỗi connection có:

- ECDH client private key riêng
- ECDH server ephemeral key riêng
- HKDF salt riêng
- AES session key riêng

Reconnect cũng tạo lại keypair mới.

Không dùng session/key của tab khác.

Frontend serialize xử lý `onmessage` bằng `messageChain` để tránh race giữa `serverHello` và event encrypted kế tiếp.

---

## 3. HTTP local API `/api/local/*`

### 3.1 File liên quan

Backend:

- `src/api/crypto.rs`
- `src/api/mod.rs`
- `src/websocket/mod.rs` - hiện đang giữ API crypto session registry trong `AppState`
- `src/main.rs`

Frontend:

- `web/src/apiCrypto.ts`
- `web/src/api.ts`

---

### 3.2 Scope mã hóa API

Mã hóa hiện áp dụng cho request từ local UI đi vào:

```text
/api/local/*
```

Các endpoint sau hiện không nằm trong encrypted local API flow:

```text
/api/health
/api/info
```

Request có:

```text
X-ChatCmdClient: chatgpt-extension
```

hiện được bypass để giữ compatibility cho ChatGPT extension.

Skill icon cũng cần lưu ý riêng vì browser `<img src=...>` không đi qua helper `api.ts`; khi sửa crypto middleware phải kiểm tra endpoint icon không bị phá.

---

### 3.3 API handshake endpoint

Endpoint:

```text
POST /api/local/crypto/handshake
```

Header client:

```text
X-ChatCmdClient: local-ui
Content-Type: application/octet-stream
```

Handshake body là binary AES-GCM packet.

Logical payload trước mã hóa:

```json
{
  "type": "crypto.clientHello",
  "protocol": 1,
  "publicKey": "..."
}
```

---

### 3.4 API handshake crypto constants

Protocol:

```text
1
```

Handshake AAD:

```text
chatcmd/api/handshake-obfuscation/v1
```

HKDF info:

```text
chatcmd/api/aes-256-gcm/v1
```

Fixed handshake key hiện dùng cùng hai byte arrays kiểu split/XOR như WebSocket.

---

### 3.5 API session flow

#### Client

`apiCrypto.ts` lazy-create một session khi request API đầu tiên chạy.

Nó tạo ECDH P-256 keypair mới, gửi encrypted `clientHello`, decrypt `serverHello`, rồi derive AES-256-GCM key.

Session chỉ nằm trong module memory:

```text
sessionPromise
```

Reload page/tab -> JS context mới -> session mới.

Mỗi tab giữ session riêng.

---

#### Server

Server trả encrypted `serverHello` chứa:

```json
{
  "type": "crypto.serverHello",
  "protocol": 1,
  "sessionId": "UUID",
  "publicKey": "...",
  "salt": "..."
}
```

Backend lưu:

```text
sessionId -> Arc<Aes256Gcm>
```

trong `AppState.api_crypto_sessions`.

Registry hiện dùng `RwLock<HashMap<...>>`.

Có giới hạn mềm 512 session. Khi đạt ngưỡng, code loại một entry hiện có trước khi insert session mới.

> Khi debug memory/session lifecycle, kiểm tra logic eviction này trước nếu có rất nhiều tab/reload liên tục.

---

## 4. HTTP API request flow

Frontend API wrapper luôn gọi:

```text
encryptedApiFetch(path, init)
```

Thay vì gọi `fetch()` trực tiếp cho local API.

Headers sau handshake:

```text
X-ChatCmdClient: local-ui
X-ChatCmd-Crypto: 1
X-ChatCmd-Crypto-Session: <session UUID>
```

Nếu body ban đầu là JSON string, frontend mã hóa body và đổi:

```text
Content-Type: application/octet-stream
```

GET/DELETE không có body thì không cần tạo ciphertext body, nhưng response vẫn được mã hóa.

---

## 5. HTTP API response flow

Backend middleware:

```text
crypto::encrypted_local_api
```

chạy quanh local routes.

Flow:

```text
client encrypted request
  -> middleware xác thực crypto headers/session
  -> decrypt body
  -> restore Content-Type: application/json
  -> gọi handler API cũ
  -> lấy handler response body
  -> encrypt response body
  -> Content-Type: application/octet-stream
  -> X-ChatCmd-Crypto: 1
  -> gửi client
```

Frontend kiểm tra:

```text
X-ChatCmd-Crypto: 1
```

Nếu có, nó đọc `arrayBuffer()`, decrypt AES-GCM, sau đó `JSON.parse()`.

Nếu status là `204 No Content`, frontend trả về ngay không decode body.

---

## 6. API AES-GCM packet format

Giống WebSocket:

```text
byte 0      : protocol version = 1
byte 1..12  : random nonce
byte 13..N  : ciphertext + GCM tag
```

---

## 7. API AAD - cực kỳ quan trọng khi debug

Khác WebSocket, API bind ciphertext vào HTTP metadata bằng AAD.

Request AAD:

```text
chatcmd/api/v1|request|<METHOD>|<FULL_PATH_AND_QUERY>
```

Ví dụ:

```text
chatcmd/api/v1|request|POST|/api/local/tasks/task-123/stop
```

Response AAD:

```text
chatcmd/api/v1|response|<METHOD>|<FULL_PATH_AND_QUERY>|<STATUS>
```

Ví dụ:

```text
chatcmd/api/v1|response|GET|/api/local/tasks?limit=10|200
```

Điều này làm ciphertext bị bind với:

- direction
- HTTP method
- original full path
- query string
- response status

Do đó packet của endpoint A không thể copy sang endpoint B rồi decrypt hợp lệ.

---

## 8. Axum `Router::nest` và `OriginalUri`

Đây là bug đã từng xảy ra và cần nhớ khi debug.

Frontend dùng path đầy đủ:

```text
/api/local/overview
```

Nhưng middleware nằm bên trong nested router có thể thấy URI đã strip prefix, ví dụ:

```text
/overview
```

Nếu backend tạo AAD bằng `/overview` nhưng frontend tạo AAD bằng `/api/local/overview`, AES-GCM authentication sẽ fail.

Triệu chứng phía UI từng là:

```text
Không thể tải dữ liệu
Yêu cầu thất bại (200)
```

HTTP vẫn `200` vì handler chạy thành công, nhưng frontend không decrypt được response và `payload === undefined`.

Fix hiện tại:

Backend ưu tiên lấy:

```rust
OriginalUri
```

để reconstruct full original path + query trước khi tạo AAD.

Fallback mới dùng current `request.uri()`.

### Checklist nếu lại gặp `Yêu cầu thất bại (200)`

Kiểm tra theo thứ tự:

1. Response có header `X-ChatCmd-Crypto: 1` không?
2. Frontend và backend có đang dùng cùng session ID không?
3. AES session key có đúng không?
4. AAD direction có đúng `request` / `response` không?
5. METHOD có uppercase giống nhau không?
6. Path có phải full `/api/local/...` ở cả hai bên không?
7. Query string có giống chính xác, bao gồm ordering và percent encoding không?
8. Response status dùng trong AAD có đúng không?
9. Packet byte 0 có protocol `1` không?
10. Body có bị middleware khác transform/compress sau khi encrypt không?

---

## 9. API session reset/retry

Nếu backend restart, `api_crypto_sessions` trong RAM mất hết nhưng tab frontend vẫn giữ session cũ.

Middleware sẽ trả response có:

```text
X-ChatCmd-Crypto-Reset: 1
```

Frontend khi thấy header này sẽ:

```text
resetApiCryptoSession()
-> handshake lại
-> retry request đúng 1 lần
```

Không retry vô hạn.

Nếu retry lần 2 vẫn fail, lỗi được propagate lên UI.

---

## 10. Error response

Nếu session hợp lệ nhưng encrypted request body sai, server cố trả ProblemDetails cũng dưới dạng encrypted response.

Ví dụ logical payload:

```json
{
  "type": "about:blank",
  "title": "API encryption error",
  "status": 400,
  "detail": "Request encryption is invalid"
}
```

Nếu server chưa có session hợp lệ hoặc cần reset session, response reset có thể là plaintext ProblemDetails kèm:

```text
X-ChatCmd-Crypto-Reset: 1
```

Frontend phải hỗ trợ cả encrypted error response và reset/plaintext error path.

---

## 11. Những gì vẫn nhìn thấy trong DevTools

Encryption layer này không che toàn bộ HTTP metadata.

Vẫn có thể thấy:

- HTTP method
- URL
- route
- query string
- status code
- request/response headers
- payload size
- timing

Phần được che là JSON body và WebSocket application payload.

Nếu muốn che path/query thì phải thiết kế một envelope endpoint/router khác, ví dụ toàn bộ request POST vào một endpoint duy nhất rồi route logic nằm trong encrypted payload. Hiện dự án **không làm như vậy**.

---

## 12. Security model và giới hạn

### Có bảo vệ

- Không lộ JSON payload trực tiếp trong Network panel.
- Mỗi connection/session có AES key riêng.
- Reload/tab khác không reuse session key.
- AES-GCM cung cấp integrity/authentication cho ciphertext.
- HTTP AAD bind packet với method/path/status.
- WebSocket plaintext application frame bị reject sau handshake.
- WebCrypto private/session keys là non-extractable.

### Không thể bảo vệ tuyệt đối

Browser owner vẫn có thể:

- hook `crypto.subtle.encrypt/decrypt`
- breakpoint trước khi encrypt hoặc sau decrypt
- patch bundle/runtime
- inspect memory/runtime objects

Fixed AES handshake key chỉ là obfuscation.

Không được mô tả fixed handshake key như một secret chống reverse-engineering tuyệt đối.

---

## 13. Các test nên chạy sau khi sửa crypto

### Rust

```bash
cargo check
cargo test api::crypto::tests
cargo test websocket::tests
```

Nếu sửa router/middleware nhiều, nên chạy full:

```bash
cargo test
```

### Frontend

Trên Windows, do repo path canonical có thể hiện dạng `\\?\D:\...`, dùng:

```cmd
cd /d D:\DEV\CmdGPT\ChatCmdClient\web && npm run build
```

Realtime tests:

```cmd
cd /d D:\DEV\CmdGPT\ChatCmdClient\web && npx vitest --run src/test/realtime.test.tsx
```

Nếu sửa API crypto, cần chạy thêm API/App tests sau khi test mocks được cập nhật theo encrypted protocol.

---

## 14. Debug workflow khuyến nghị cho AI

Khi user báo API/WebSocket encryption lỗi, không nên đoán ngay AES key sai.

Nên kiểm tra theo flow này.

### WebSocket

1. Xác nhận frame đầu là binary.
2. Check client/server handshake AAD giống nhau.
3. Check fixed key A/B giống nhau.
4. Check ECDH curve đều P-256.
5. Check raw public key Base64URL encoding/decoding.
6. Check HKDF salt và `info` giống nhau.
7. Check application AAD `chatcmd/ws/v1`.
8. Check packet format offset `1 + 12`.
9. Check browser `binaryType = arraybuffer`.
10. Check message ordering/race (`messageChain`).

### HTTP API

1. Xác nhận `/api/local/crypto/handshake` thành công.
2. Xác nhận response handshake decrypt được.
3. Xác nhận `sessionId` tồn tại backend.
4. Xác nhận request có `X-ChatCmd-Crypto-Session` đúng.
5. Check full path bằng `OriginalUri`.
6. Check query string exact match.
7. Check method uppercase.
8. Check response status included trong response AAD.
9. Check body chưa bị đọc trước crypto middleware.
10. Check middleware ordering.
11. Nếu backend vừa restart, check reset header + one-time retry.
12. Nếu HTTP 200 nhưng UI fail, ưu tiên nghi **response decrypt/AAD mismatch**.

---

## 15. Middleware ordering

Crypto layer phải chạy sao cho encrypted body được decrypt **trước** khi Axum JSON extractor của API handler đọc body.

Response phải được encrypt **sau** khi handler đã tạo JSON response.

Nếu đổi `.layer(...)` order trong `src/api/mod.rs`, phải kiểm tra lại ngay vì Axum middleware ordering có thể đảo kỳ vọng nếu không để ý.

`management_header` vẫn cần nhận được header `X-ChatCmdClient`.

`crypto/handshake` phải được bypass khỏi encrypted-session middleware, vì chính endpoint đó dùng để tạo session.

---

## 16. Không được vô tình mã hóa binary resource như JSON

Crypto middleware local UI hiện được thiết kế chủ yếu cho JSON API body.

Khi thêm endpoint mới trả:

- image
- file download
- stream
- SSE
- multipart
- arbitrary binary

phải quyết định rõ:

1. bypass crypto middleware, hoặc
2. mở rộng protocol để hỗ trợ binary content type đúng cách.

Không được tự động đổi mọi decrypted request body thành `application/json` nếu endpoint mới không phải JSON.

---

## 17. Các invariants cần giữ khi refactor

### WebSocket

- Handshake frame luôn binary.
- Handshake fixed AES chỉ dùng trước session.
- ECDH secret là ephemeral per connection.
- Session key không persist.
- Application frame luôn AES-GCM binary.
- Plaintext application text frame bị reject.

### API

- Mỗi browser JS context có independent session.
- Session backend nằm RAM-only.
- Full original path + query được dùng cho AAD.
- Request/response sử dụng direction khác nhau trong AAD.
- Response AAD chứa status code.
- 204 không có encrypted response body.
- Backend restart phải trigger reset + one-time re-handshake/retry.
- `chatgpt-extension` compatibility không được vô tình phá nếu chưa migrate extension sang protocol mới.

---

## 18. Tóm tắt nhanh

WebSocket:

```text
Browser
  -> binary AES fixed-key clientHello
Server
  -> binary AES fixed-key serverHello
Both
  -> ECDH P-256
  -> HKDF-SHA256
  -> AES-256-GCM session
  -> all WS app frames encrypted binary
```

HTTP API:

```text
Browser tab
  -> POST /api/local/crypto/handshake
  -> binary AES fixed-key clientHello
Server
  -> sessionId + encrypted serverHello
Both
  -> ECDH P-256
  -> HKDF-SHA256
  -> AES-256-GCM session

Then each /api/local request:
  request JSON -> AES-GCM binary
  handler receives decrypted JSON
  handler JSON response -> AES-GCM binary
  browser decrypts -> JSON.parse
```

Nếu gặp lỗi mà HTTP status vẫn `200` nhưng UI báo không tải được dữ liệu, kiểm tra **AAD/path/session/decrypt** trước tiên.

---

## 19. Backend server API: React -> local API -> ChatCMD.Api

Backend server không được gọi trực tiếp từ React. Luồng bắt buộc:

```text
React
  -> /api/local/backend/*
  -> local API encryption (browser <-> ChatCmdClient)
  -> Rust BackendApiClient
  -> backend API encryption (ChatCmdClient <-> ChatCMD.Api)
  -> /api/* trên ChatCMD.Api
```

Frontend helper nằm trong `web/src/api.ts` với `backendApi.get/post/put/patch/delete`. Helper chỉ tạo URL local `/api/local/backend/...`; domain/port backend không tồn tại trong React bundle.

Local gateway nằm tại `src/api/backend.rs`. Ví dụ:

```text
React:       GET /api/local/backend/system/ping
Rust proxy:  GET /api/system/ping
Backend:     ChatCMD.Api
```

Rust chỉ forward các header đã allowlist, hiện gồm `Authorization` và `Accept-Language`. Không forward toàn bộ browser headers.

### Backend crypto protocol

Handshake:

```text
POST /api/crypto/handshake
binary fixed AES-GCM obfuscation
-> ephemeral ECDH P-256
-> HKDF-SHA256
-> AES-256-GCM session
```

Handshake AAD:

```text
chatcmd/backend-api/handshake-obfuscation/v1
```

HKDF info:

```text
chatcmd/backend-api/aes-256-gcm/v1
```

Request AAD:

```text
chatcmd/backend-api/v1|request|<METHOD>|<FULL_PATH_AND_QUERY>
```

Response AAD:

```text
chatcmd/backend-api/v1|response|<METHOD>|<FULL_PATH_AND_QUERY>|<STATUS>
```

Backend packet format vẫn là protocol byte `1` + nonce 12 byte + ciphertext/GCM tag.

`ChatCMD.Api` lưu session key trong RAM theo `sessionId`. Nếu backend restart hoặc session hết hạn, server trả `X-ChatCmd-Crypto-Reset: 1`; Rust xóa session, handshake lại và retry request đúng một lần.

### Backend URL đóng cứng trong source

Backend URL được quyết định trực tiếp trong `src/backend_api.rs`:

```rust
const DEBUG_BACKEND_API_URL: &str = "http://127.0.0.1:5121";
const RELEASE_BACKEND_API_URL: &str = "https://api.chatcmd.net";
```

Debug build dùng `DEBUG_BACKEND_API_URL`; release build dùng `RELEASE_BACKEND_API_URL`. Bản đóng gói không đọc biến môi trường để override backend URL, nên endpoint production được cố định ngay trong binary. Trước khi public chỉ cần sửa `RELEASE_BACKEND_API_URL` rồi chạy script build.

### Debug backend interop

Backend dev hiện dùng launch profile HTTP port `5121`.

Chạy backend:

```powershell
cd D:\DEV\ChatCMD\ChatCMD
dotnet run --project ChatCMD.Api\ChatCMD.Api.csproj --launch-profile http
```

Test trực tiếp Rust <-> .NET encrypted protocol:

```cmd
set CHATCMD_TEST_BACKEND_INTEROP=1
cargo test backend_api::tests::local_dotnet_backend_interop_when_enabled -- --nocapture
```

Nếu test này fail thì debug lớp app-to-backend trước, chưa cần kiểm tra React/local API. Các điểm cần so sánh hai phía: fixed handshake key fragments, P-256 raw public key format, HKDF salt/info, AAD path/query/method/status và packet offsets.
