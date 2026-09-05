//! Exercise the real nested API router, including extension authorization.
use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

use crate::{runtime_host::user_message_tests::test_host, websocket::AppState};

const CRYPTO_KEY: [u8; 32] = [7; 32];
const CRYPTO_SESSION: &str = "bridge-router-test-session";

pub(super) async fn fixture(status: &str) -> (Arc<AppState>, Router, TempDir) {
    let (host, agent_id, directory) = test_host().await;
    let state =
        Arc::new(host.test_app_state(directory.path().join("chatcmd.db").display().to_string()));
    let now = super::now_ms();
    // Identical prompt text must not cause two request identities to be conflated.
    for suffix in ["a", "b"] {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,'Bridge router test','chatgpt_web',?,1,?,?)")
            .bind(format!("task-{suffix}")).bind(&agent_id).bind(state.device.id.as_str())
            .bind(status).bind(now).bind(now).execute(state.repository.pool()).await.expect("seed task");
        sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'Auto','xin chào','xin chào',?,?,?)")
            .bind(format!("request-{suffix}")).bind(format!("task-{suffix}"))
            .bind(format!("turn-{suffix}")).bind(&agent_id).bind(status).bind(now).bind(now)
            .execute(state.repository.pool()).await.expect("seed request without identity");
    }
    state
        .put_api_crypto_session(
            CRYPTO_SESSION.to_owned(),
            Aes256Gcm::new_from_slice(&CRYPTO_KEY).expect("test cipher"),
        )
        .await;
    // Match main.rs -> api::router -> /local nesting, not a direct handler call.
    let app = Router::new()
        .nest("/api", super::router(state.clone()))
        .with_state(state.clone());
    (state, app, directory)
}

pub(super) async fn extension_request(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("X-ChatCmdClient", "chatgpt-extension")
                .header("Content-Type", "application/json")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .expect("request"),
        )
        .await
        .expect("router response")
}

pub(super) async fn expect_json(response: Response, status: StatusCode) -> Value {
    let actual = response.status();
    let body = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("response body");
    assert_eq!(
        actual,
        status,
        "response: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("JSON response")
}

pub(super) async fn gui_get(app: &Router, path: &str, cookie: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header("X-ChatCmdClient", "local-ui")
        .header("X-Chatcmd-Crypto", "1")
        .header("X-Chatcmd-Crypto-Session", CRYPTO_SESSION);
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("GUI request"))
        .await
        .expect("GUI response");
    let status = response.status();
    assert_eq!(response.headers()["x-chatcmd-crypto"], "1");
    let packet = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("encrypted response");
    assert_eq!(packet[0], 1);
    let aad = format!("chatcmd/api/v1|response|GET|{path}|{}", status.as_u16());
    let cipher = Aes256Gcm::new_from_slice(&CRYPTO_KEY).expect("test cipher");
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&packet[1..13]),
            Payload {
                msg: &packet[13..],
                aad: aad.as_bytes(),
            },
        )
        .expect("decrypt GUI response using original path");
    (
        status,
        serde_json::from_slice(&plaintext).expect("GUI JSON"),
    )
}

#[tokio::test]
async fn completed_requests_accept_identity_through_nested_router() {
    let (state, app, _directory) = fixture("completed").await;
    let token = state
        .gui_auth
        .setup_password("router-test-password".to_owned())
        .await
        .expect("GUI login");
    let cookie = format!("chatcmd_gui_session={token}");
    for suffix in ["a", "b"] {
        let id = format!("conversation-{suffix}");
        let url = format!("https://chatgpt.com/g/g-p-test/c/{id}");
        let path = format!("/api/local/chatgpt/bridge/request-{suffix}/identity");
        for _ in 0..2 {
            let response = extension_request(
                &app,
                "POST",
                &path,
                json!({ "conversationId": id, "conversationUrl": url }),
            )
            .await;
            let request = expect_json(response, StatusCode::OK).await;
            assert_eq!(request["conversationUrl"], url);
            assert_eq!(request["status"], "completed");
        }
        let read_path = format!("/api/local/chatgpt/requests/request-{suffix}?probe=1");
        let request = expect_json(
            extension_request(&app, "GET", &read_path, Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(request["taskId"], format!("task-{suffix}"));
        let (status, bridge) = gui_get(
            &app,
            &format!("/api/local/chatgpt/tasks/task-{suffix}"),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bridge["conversationId"], id);
        assert_eq!(bridge["conversationUrl"], url);
        assert_eq!(bridge["taskStatus"], "completed");
        assert!(bridge["activeRequestId"].is_null());
    }
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_conversations")
        .fetch_one(state.repository.pool())
        .await
        .expect("conversation count");
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn started_and_result_callbacks_reach_handlers_through_nested_router() {
    let (state, app, _directory) = fixture("running").await;
    let identity = json!({ "conversationId": "conversation-started", "conversationUrl": "https://chatgpt.com/c/conversation-started" });
    let started = extension_request(
        &app,
        "POST",
        "/api/local/chatgpt/bridge/request-a/started",
        identity,
    )
    .await;
    let request = expect_json(started, StatusCode::OK).await;
    assert_eq!(request["taskId"], "task-a");
    let result = extension_request(&app, "POST", "/api/local/chatgpt/bridge/request-a/result", json!({
        "status": "completed", "assistantContent": "Test response",
        "conversationId": "conversation-started", "conversationUrl": "https://chatgpt.com/c/conversation-started"
    })).await;
    assert_eq!(
        expect_json(result, StatusCode::OK).await["status"],
        "completed"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id='task-a'")
        .fetch_one(state.repository.pool())
        .await
        .expect("task status");
    assert_eq!(status, "completed");
    let browser_result = extension_request(&app, "POST", "/api/local/chatgpt/bridge/request-b/browser-completed", json!({
        "assistantContent": "Other response", "conversationId": "conversation-browser", "conversationUrl": "https://chatgpt.com/c/conversation-browser"
    })).await;
    assert_eq!(
        expect_json(browser_result, StatusCode::OK).await["status"],
        "completed"
    );
}

#[tokio::test]
async fn extension_allowlist_still_denies_management_and_wrong_actions() {
    let (_state, app, _directory) = fixture("completed").await;
    for (method, path) in [
        ("GET", "/api/local/settings"),
        ("GET", "/api/local/mcp/agents"),
        ("GET", "/api/local/chatgpt/tasks/task-a"),
        ("POST", "/api/local/chatgpt/requests"),
        ("POST", "/api/local/tasks/task-a/stop"),
        ("DELETE", "/api/local/tasks/task-a"),
        (
            "GET",
            "/api/local/settings?path=/api/local/chatgpt/requests/request-a",
        ),
    ] {
        let response = extension_request(&app, method, path, json!({})).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    let spoofed = Request::builder()
        .uri("/api/local/settings")
        .header("X-ChatCmdClient", "chatgpt-extension")
        .header("X-Original-Uri", "/api/local/chatgpt/requests/request-a")
        .body(Body::empty())
        .expect("spoofed request");
    assert_eq!(
        app.oneshot(spoofed).await.expect("response").status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn extension_approval_and_subagent_routes_survive_nesting() {
    let (_state, app, _directory) = fixture("completed").await;
    for path in [
        "/api/local/tasks/approvals/pending",
        "/api/local/tasks/activity-approvals/pending",
        "/api/local/plan/questions/pending",
    ] {
        let response = extension_request(&app, "GET", path, Value::Null).await;
        expect_json(response, StatusCode::OK).await;
    }
    for action in ["started", "result"] {
        let path = format!("/api/local/subagents/missing/fallback/{action}");
        let response = extension_request(
            &app,
            "POST",
            &path,
            json!({ "attempt": 1, "status": "completed" }),
        )
        .await;
        // Missing fixtures must reach the handler's 404, not middleware's 403.
        expect_json(response, StatusCode::NOT_FOUND).await;
    }
}

#[tokio::test]
async fn gui_session_and_api_encryption_remain_required() {
    let (_state, app, _directory) = fixture("completed").await;
    let path = "/api/local/chatgpt/tasks/task-a";
    let (status, _) = gui_get(&app, path, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let plaintext = Request::builder()
        .uri(path)
        .header("X-ChatCmdClient", "local-ui")
        .body(Body::empty())
        .expect("plaintext request");
    assert_eq!(
        app.clone()
            .oneshot(plaintext)
            .await
            .expect("response")
            .status(),
        StatusCode::UPGRADE_REQUIRED
    );
    let anonymous = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("anonymous request");
    assert_eq!(
        app.oneshot(anonymous).await.expect("response").status(),
        StatusCode::FORBIDDEN
    );
}
