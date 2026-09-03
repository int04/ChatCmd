# Plan 17 — Bind MCP identity mà không đọc/parse/serialize lại toàn request body

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế lại middleware `request_identity` để authenticated agent, MCP session và private conversation scope được truyền qua trusted request extensions/session context, không cần đọc toàn JSON body rồi chèn field và serialize lại. Đồng thời giữ chống spoofing, hỗ trợ JSON-RPC batch và tương thích `rmcp`. Không commit.

## Ưu tiên

**P0 — memory, request-size architecture và security boundary.** Middleware hiện tạo nhiều bản sao payload và đặt limit 4 MiB cho mọi MCP POST, khiến whole-content write không mở rộng được.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-mcp/src/request_identity.rs:7-33`:
  - `MAX_MCP_BODY_BYTES = 4 * 1024 * 1024`;
  - `to_bytes(body, MAX_MCP_BODY_BYTES)` đọc full body;
  - parse thành `serde_json::Value`;
  - `bind_identity_value` mutate arguments;
  - `serde_json::to_vec` tạo body mới.
- `request_identity.rs:35-81` xóa client-provided identity aliases rồi chèn `agentId`, `__chatcmdMcpSessionId`, `__chatcmdConversationScopeId` vào tool arguments.
- `crates/chatcmd-mcp/src/lib.rs:380-420` `prepare_call` lấy authenticated agent/session/conversation scope từ `ToolArguments`; cần audit macro/envelope parse.
- `crates/chatcmd-mcp/src/lib.rs:780-825` `mcp_handler` authorize token/origin, tính local session, rồi gọi middleware bind trước `rmcp` service.
- Tests `request_identity.rs:112-193` xác nhận spoofed fields bị override/remove; behavior này phải được giữ.

Plan này liên quan plan 10: dữ liệu lớn nên đi qua blob/binary endpoint riêng, không đơn giản tăng JSON limit vô hạn.

## Mục tiêu

1. Trusted identity không nằm trong untrusted tool JSON arguments như nguồn chân lý.
2. Middleware HTTP không buffer/parse/serialize full MCP body chỉ để bind identity.
3. Client spoofed `agentId`/private correlation không thắng server context.
4. JSON-RPC batch, initialize/notification/tool call đều giữ đúng behavior.
5. Body limits phân theo endpoint/message type; control JSON vẫn bounded, large binary đi qua blob flow.
6. `rmcp` handler nhận identity bằng extension/task-local/session context an toàn, kể cả service xử lý request trong task khác.
7. Có test qua transport thật, không chỉ gọi helper trực tiếp.
8. Không làm lộ token path/private scope vào logs, errors hoặc tool-visible fields không cần thiết.

## Thiết kế đích đề xuất

### Trusted context

Tạo type:

```rust
#[derive(Clone)]
pub struct AuthenticatedMcpContext {
    pub agent_id: AgentId,
    pub local_session_id: McpSessionId,
    pub conversation_scope_id: Option<ConversationScopeId>,
}
```

Sau authorization, insert vào `Request::extensions_mut()` hoặc context được transport adapter hỗ trợ. Handler/tool router lấy context này, không lấy identity do client gửi.

Nếu `rmcp::StreamableHttpService` không forward request extensions vào tool handler, khảo sát các lựa chọn theo thứ tự:

1. hook/session factory nhận request context;
2. layer/service wrapper dùng task-local scope quanh toàn future xử lý request, bảo đảm context được propagate;
3. map `mcp-session-id` → authenticated context server-side trong `LocalSessionManager`;
4. patch/upstream contribution nếu library thiếu hook.

Không quay lại chèn trusted identity vào arbitrary JSON nếu có cách context an toàn. Nếu buộc giữ compatibility shim, shim chỉ parse bounded control requests và không là nguồn chân lý; ghi ADR rõ.

### Conversation scope

`openai/session` hiện nằm trong JSON-RPC `_meta`, nên vẫn cần đọc metadata nào đó. Các lựa chọn:

- Host đưa scope qua trusted HTTP header đã được tunnel/extension ký/xác thực;
- Transport/session initialize metadata được parse một lần bởi `rmcp`, sau đó hook cung cấp;
- Streaming/selective parser chỉ trích `_meta` bounded mà không materialize toàn arguments;
- Server map remote MCP session header với scope đã bind ở initialize.

Khuyến nghị bind scope tại initialize/first trusted request và lưu server-side theo authenticated session. Tool calls sau không cần parse body ở middleware. Raw client field trong `arguments` luôn bị bỏ qua bởi runtime.

## Chống spoofing

- Loại các common identity fields khỏi public tool schema nếu không cần caller gửi.
- `ToolArguments` không nên expose `agentId` như regular field; trusted context được truyền riêng.
- Nếu compatibility cần nhận field cũ, deserialize nhưng discard và metric `spoofedIdentityFieldSeen` bounded, không log value.
- Private fields bắt đầu `__chatcmd*` từ client không được map vào context.
- Session ownership bind với token-authenticated agent; header session ID được hash/namespaced như hiện tại.
- Batch request có mỗi item nhưng cùng trusted HTTP/session context; conversation scope per-item chỉ được nhận từ trusted metadata policy.

## Body limits/streaming

- Giữ hard cap cho JSON-RPC control body, ví dụ 1–4 MiB, nhưng lý do là chống abuse, không dùng nó làm file transfer.
- Kiểm tra `Content-Length` sớm khi có; vẫn enforce streaming cap khi chunked.
- Không gọi `to_bytes` ở identity middleware.
- `rmcp` có thể vẫn parse JSON body một lần; đó là cần thiết cho JSON-RPC, nhưng tránh lần parse/serialize bổ sung.
- Blob upload endpoint/tool chunk có cap riêng và binary streaming theo plan 10.
- Response large content dùng result cap/artifact, không đẩy giant JSON ngược lại.

## Các bước triển khai

1. Đọc API `rmcp` version hiện tại và xác định điểm truyền request/session context; không đoán.
2. Viết integration test hiện tại qua Axum router/transport để giữ auth/origin/session/spoof behavior.
3. Tạo `AuthenticatedMcpContext` newtypes và context propagation mechanism.
4. Sửa `prepare_call`/`ToolArguments` để lấy trusted identity riêng.
5. Thiết kế bind conversation scope tại session/initialize; thêm storage/TTL nếu cần.
6. Xóa full-body rewrite khỏi middleware.
7. Xóa/compat-discard untrusted identity fields khỏi tool schema/envelope.
8. Thêm body-limit layer phù hợp và integration với blob endpoint.
9. Thêm structured metrics cho request bytes/rejection/context source mà không log secret.
10. Cập nhật tests, docs, security comments và catalog metadata.

## Edge cases bắt buộc

- JSON-RPC single/batch, notification, initialize, malformed JSON, empty body.
- Batch chứa nhiều tool calls với spoofed identity khác nhau.
- Missing/reused/rotated MCP session header.
- Hai authenticated agents dùng cùng remote session string.
- Host không gửi `openai/session`; fallback correlation an toàn.
- Request body chunked, Content-Length sai/quá cap.
- Disconnect giữa body stream.
- Context future spawn/task switch trong `rmcp`.
- Reconnect sau app restart và session context expiry.
- Token rotation/revocation khi session đang mở.

## Test bắt buộc

- Authenticated agent luôn thắng spoofed fields; spoofed private scope không được dùng.
- Tool handler nhận đúng identity mà body không bị rewrite.
- Byte-for-byte request body đi qua identity middleware không bị consume/re-encode, hoặc integration chứng minh chỉ parser chính đọc một lần.
- Batch identity consistency.
- Two-agent same session header isolation.
- Conversation scope stable cùng chat và khác giữa chat.
- Body cap/malformed/empty/non-POST behavior.
- Large blob control flow không vượt JSON cap và không qua full-body identity parser.
- Test memory/allocation hoặc instrumentation cho request gần cap, chứng minh bỏ một parse + serialization + copy.
- Real packaged/streamable HTTP connection regression.

## Tiêu chí nghiệm thu

- `bind_authenticated_agent` không còn `to_bytes` + parse `Value` + `to_vec` cho mọi POST.
- Trusted identity đi qua server-owned context, không qua caller-controlled arguments.
- Spoofing tests cũ vẫn pass hoặc được thay bằng test mạnh hơn.
- Conversation scope correlation không bị mất.
- JSON control cap vẫn fail closed; large content dùng blob path riêng.
- Integration test dùng Axum + rmcp transport thật.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-mcp
cargo test --workspace
```

## Kết quả AI phải trả về

- Cách `rmcp` context được truyền thực tế.
- Security model trước/sau.
- File đã đổi và compatibility fields.
- Conversation-scope session flow.
- Body limits/blob interaction.
- Integration/performance test result.
