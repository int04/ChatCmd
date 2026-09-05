use super::result_contract_tests::{
    CountingBody, DisconnectingBody, MutableAuth, RecordingRuntime, TokenAuth,
};
use super::*;

fn mcp_post(path: &str, body: impl Into<Body>, session_id: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "localhost")
        .header("origin", "https://allowed.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(session_id) = session_id {
        builder = builder
            .header("mcp-session-id", session_id)
            .header("mcp-protocol-version", "2025-03-26");
    }
    builder.body(body.into()).expect("MCP request")
}

fn mcp_delete(path: &str, session_id: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("host", "localhost")
        .header("origin", "https://allowed.example")
        .header("mcp-session-id", session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .body(Body::empty())
        .expect("MCP delete request")
}

async fn initialize_existing_router(router: &Router, token: &str) -> String {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "identity-test", "version": "1"}
        }
    })
    .to_string();
    let response = router
        .clone()
        .oneshot(mcp_post(&format!("/mcp/{token}"), initialize, None))
        .await
        .expect("initialize response");
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body");
        panic!(
            "initialize failed with {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session header")
        .to_owned();
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
    .to_string();
    let response = router
        .clone()
        .oneshot(mcp_post(
            &format!("/mcp/{token}"),
            initialized,
            Some(&session_id),
        ))
        .await
        .expect("initialized response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    session_id
}

async fn initialized_router(runtime: RecordingRuntime, agent: &str) -> (Router, String) {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router =
        axum_router_with_host_validation(McpServer::new(Arc::new(runtime)), security, false);
    let session_id = initialize_existing_router(&router, agent).await;
    (router, session_id)
}

#[tokio::test]
async fn streamable_http_uses_trusted_identity_and_parsed_request_meta() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "device_list",
            "_meta": {"openai/session": "chat-a"},
            "arguments": {
                "agentId": "spoofed-agent",
                "__chatcmdMcpSessionId": "spoofed-session",
                "__chatcmdConversationScopeId": "spoofed-scope"
            }
        }
    })
    .to_string();
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", call, Some(&session_id)))
        .await
        .expect("tool response");
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("tool response body");

    let calls = runtime.calls.lock().expect("recorded calls");
    let (context, arguments) = calls.first().unwrap_or_else(|| {
        panic!(
            "runtime tool call; response was {}",
            String::from_utf8_lossy(&response_body)
        )
    });
    assert_eq!(context.agent_id, "agent-a");
    assert_eq!(
        context.mcp_session_id.as_deref(),
        Some(local_mcp_session_id("agent-a", Some(&session_id)).as_str())
    );
    assert!(
        context
            .conversation_scope_id
            .as_deref()
            .is_some_and(|scope| scope.starts_with("openai:"))
    );
    assert_eq!(arguments, &serde_json::json!({}));
}

#[tokio::test]
async fn streamable_http_session_cannot_be_reused_by_another_agent() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime, "agent-a").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();
    let response = router
        .oneshot(mcp_post("/mcp/agent-b", call, Some(&session_id)))
        .await
        .expect("cross-agent response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn streamable_http_rejects_empty_and_malformed_control_json() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );

    let empty = router
        .clone()
        .oneshot(mcp_post("/mcp/agent-a", "", None))
        .await
        .expect("empty control response");
    assert_eq!(empty.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let malformed = router
        .oneshot(mcp_post("/mcp/agent-a", "{not-json", None))
        .await
        .expect("malformed control response");
    assert_eq!(malformed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn streamable_http_delete_terminates_owned_session() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime, "agent-a").await;
    let response = router
        .oneshot(mcp_delete("/mcp/agent-a", &session_id))
        .await
        .expect("delete response");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn streamable_http_rejects_declared_control_body_over_cap_before_polling_body() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let mut request = mcp_post("/mcp/agent-a", "{}", None);
    request.headers_mut().insert(
        http::header::CONTENT_LENGTH,
        http::HeaderValue::from_str(&(request_identity::MCP_CONTROL_BODY_BYTES + 1).to_string())
            .expect("content length"),
    );
    let response = router
        .oneshot(request)
        .await
        .expect("declared oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn streamable_http_uses_actual_streamed_bytes_when_content_length_is_too_small() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let oversized = vec![b'x'; request_identity::MCP_CONTROL_BODY_BYTES + 1];
    let mut request = mcp_post("/mcp/agent-a", oversized, None);
    request.headers_mut().insert(
        http::header::CONTENT_LENGTH,
        http::HeaderValue::from_static("1"),
    );
    let response = router
        .oneshot(request)
        .await
        .expect("actual oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn streamable_http_body_disconnect_fails_closed() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let body = Body::new(DisconnectingBody { state: 0 });
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", body, None))
        .await
        .expect("disconnect response");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn streamable_http_revoked_token_cannot_reuse_active_session_and_rotation_can() {
    let auth = MutableAuth::default();
    auth.allow("token-old", "agent-a");
    let runtime = RecordingRuntime::default();
    let security = HttpSecurity::new(Arc::new(auth.clone()), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(runtime.clone())),
        security,
        false,
    );
    let session_id = initialize_existing_router(&router, "token-old").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();

    auth.revoke("token-old");
    let revoked = router
        .clone()
        .oneshot(mcp_post("/mcp/token-old", call.clone(), Some(&session_id)))
        .await
        .expect("revoked response");
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    auth.allow("token-new", "agent-a");
    let rotated = router
        .oneshot(mcp_post("/mcp/token-new", call, Some(&session_id)))
        .await
        .expect("rotated response");
    assert_eq!(rotated.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(rotated.into_body(), 64 * 1024)
        .await
        .expect("rotated response body");
    assert_eq!(runtime.calls.lock().expect("recorded calls").len(), 1);
}

#[tokio::test]
async fn streamable_http_restart_invalidates_old_remote_session() {
    let runtime = RecordingRuntime::default();
    let (first_router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    drop(first_router);

    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let restarted =
        axum_router_with_host_validation(McpServer::new(Arc::new(runtime)), security, false);
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();
    let stale = restarted
        .clone()
        .oneshot(mcp_post("/mcp/agent-a", call, Some(&session_id)))
        .await
        .expect("stale session response");
    assert_eq!(stale.status(), StatusCode::NOT_FOUND);

    let fresh_session = initialize_existing_router(&restarted, "agent-a").await;
    assert_ne!(fresh_session, session_id);
}

#[tokio::test]
async fn streamable_http_near_cap_body_is_consumed_once_by_transport() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    let padding = "x".repeat(request_identity::MCP_CONTROL_BODY_BYTES - 4096);
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "device_list",
            "arguments": {"padding": padding}
        }
    })
    .to_string();
    assert!(call.len() < request_identity::MCP_CONTROL_BODY_BYTES);
    assert!(call.len() > request_identity::MCP_CONTROL_BODY_BYTES - 8192);

    let bytes_polled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let body = Body::new(CountingBody {
        data: Some(axum::body::Bytes::from(call.clone())),
        bytes_polled: bytes_polled.clone(),
    });
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", body, Some(&session_id)))
        .await
        .expect("near-cap tool response");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("near-cap response body");
    assert_eq!(
        bytes_polled.load(std::sync::atomic::Ordering::Relaxed),
        call.len(),
        "identity middleware must not pre-consume or replay the request body"
    );
    assert_eq!(runtime.calls.lock().expect("recorded calls").len(), 1);
}

#[tokio::test]
async fn streamable_http_enforces_control_body_cap_without_content_length_header() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let oversized = vec![b'x'; request_identity::MCP_CONTROL_BODY_BYTES + 1];
    let request = mcp_post("/mcp/agent-a", oversized, None);
    assert!(
        request
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .is_none()
    );
    let response = router.oneshot(request).await.expect("oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
