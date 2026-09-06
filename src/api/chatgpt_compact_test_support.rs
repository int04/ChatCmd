use super::chatgpt_router_tests::{expect_json, extension_request};
use crate::websocket::AppState;
use axum::{
    Router,
    body::Body,
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

/// Exercise a plaintext JSON GUI request and response through the real router.
pub(super) async fn gui_post(
    app: &Router,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("X-ChatCmdClient", "local-ui")
        .header("Content-Type", "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header("Cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).expect("JSON"))
}
