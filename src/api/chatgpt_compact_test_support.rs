use super::chatgpt_router_tests::{expect_json, extension_request};
use crate::websocket::AppState;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

pub(super) const TASK_PATH: &str = "/api/local/tasks/task-a/chatgpt/compact";

pub(super) async fn fixture() -> (Arc<AppState>, Router, TempDir) {
    let (state, app, dir) = super::chatgpt_router_tests::fixture("running").await;
    for suffix in ["a", "b"] {
        let id = format!("old-chat-{suffix}");
        let url = format!("https://chatgpt.com/g/g-p-project/c/{id}");
        sqlx::query(
            "UPDATE chatgpt_bridge_requests SET conversation_id=?,conversation_url=? WHERE id=?",
        )
        .bind(&id)
        .bind(&url)
        .bind(format!("request-{suffix}"))
        .execute(state.repository.pool())
        .await
        .expect("seed request identity");
        sqlx::query("UPDATE tasks SET conversation_scope_hash=?,project_folder='D:\\project',allow_execute=0 WHERE id=?")
            .bind(chatcmd_storage::compact::openai_scope(&id)).bind(format!("task-{suffix}"))
            .execute(state.repository.pool()).await.expect("scope and permissions");
        sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Pro',?,1,1)")
            .bind(format!("task-{suffix}")).bind(&id).bind(&url).bind(format!("request-{suffix}"))
            .execute(state.repository.pool()).await.expect("seed binding");
    }
    (state, app, dir)
}

pub(super) async fn start(app: &Router) -> Value {
    expect_json(
        extension_request(app, "POST", TASK_PATH, json!({})).await,
        StatusCode::OK,
    )
    .await
}

pub(super) async fn checkpoint(
    app: &Router,
    job: &Value,
    extra: Value,
    status: StatusCode,
) -> Value {
    let path = format!(
        "/api/local/chatgpt/compact/{}/checkpoint",
        job["id"].as_str().expect("job id")
    );
    let mut input = json!({"expectedRevision":job["revision"]});
    input
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("extra fields").clone());
    expect_json(extension_request(app, "POST", &path, input).await, status).await
}

pub(super) async fn ready(app: &Router) -> Value {
    let job = start(app).await;
    let job = checkpoint(
        app,
        &job,
        json!({"phase":"writing_handoff"}),
        StatusCode::OK,
    )
    .await;
    let job = checkpoint(
        app,
        &job,
        json!({"phase":"saving_handoff","handoffText":"Durable transfer context"}),
        StatusCode::OK,
    )
    .await;
    checkpoint(
        app,
        &job,
        json!({"phase":"opening_new_chat"}),
        StatusCode::OK,
    )
    .await
}

pub(super) async fn complete(app: &Router, job: &Value) -> Value {
    checkpoint(app,job,json!({"phase":"completed","newConversationId":"new-chat","newConversationUrl":"https://chatgpt.com/g/g-p-project/c/new-chat"}),StatusCode::OK).await
}

pub(super) async fn cookie(state: &AppState) -> String {
    let token = state
        .gui_auth
        .setup_password("compact-router-password".to_owned())
        .await
        .expect("GUI setup");
    format!("chatcmd_gui_session={token}")
}

/// Exercise both encrypted request and encrypted response with the original URL AAD.
pub(super) async fn gui_post(
    app: &Router,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let cipher = Aes256Gcm::new_from_slice(&[7; 32]).expect("test cipher");
    let nonce_id = uuid::Uuid::new_v4();
    let nonce = Nonce::from_slice(&nonce_id.as_bytes()[..12]);
    let aad = format!("chatcmd/api/v1|request|POST|{path}");
    let encrypted = cipher
        .encrypt(
            nonce,
            Payload {
                msg: body.to_string().as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .expect("encrypt request");
    let mut packet = vec![1];
    packet.extend_from_slice(nonce);
    packet.extend(encrypted);
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("X-ChatCmdClient", "local-ui")
        .header("X-Chatcmd-Crypto", "1")
        .header("X-Chatcmd-Crypto-Session", "bridge-router-test-session");
    if let Some(cookie) = cookie {
        builder = builder.header("Cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(packet)).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    assert_eq!(response.headers()["x-chatcmd-crypto"], "1");
    let packet = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let aad = format!("chatcmd/api/v1|response|POST|{path}|{}", status.as_u16());
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&packet[1..13]),
            Payload {
                msg: &packet[13..],
                aad: aad.as_bytes(),
            },
        )
        .expect("decrypt response");
    (status, serde_json::from_slice(&plain).expect("JSON"))
}
