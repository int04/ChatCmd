use super::chatgpt_compact_test_support::*;
use super::chatgpt_router_tests::{expect_json, extension_request};
use axum::http::StatusCode;
use chatcmd_runtime::OperationContext;
use serde_json::json;

#[tokio::test]
async fn compact_work_resume_requires_attachment_and_is_idempotent_on_same_task() {
    let (state, app, _dir) = fixture().await;
    expect_json(
        extension_request(
            &app,
            "POST",
            TASK_PATH,
            json!({"continueAfterCompact":true}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let job = ready(&app).await;
    let path = format!(
        "/api/local/chatgpt/compact/{}/resume",
        job["id"].as_str().unwrap()
    );
    expect_json(
        extension_request(&app, "POST", &path, json!({})).await,
        StatusCode::CONFLICT,
    )
    .await;
    let done = complete(&app, &job).await;
    let first = expect_json(
        extension_request(&app, "POST", &path, json!({})).await,
        StatusCode::OK,
    )
    .await;
    let second = expect_json(
        extension_request(&app, "POST", &path, json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first, second);
    let id = first["requestId"]
        .as_str()
        .expect("working request created");
    let row: (String, String, String, String) = sqlx::query_as("SELECT task_id,conversation_id,status,submitted_content FROM chatgpt_bridge_requests WHERE id=?")
        .bind(id).fetch_one(state.repository.pool()).await.unwrap();
    assert_eq!(row.0, "task-a");
    assert_eq!(row.1, "new-chat");
    assert_eq!(row.2, "queued");
    assert!(row.3.contains("task-a"));
    assert!(row.3.contains("[[CHATCMD-CONTINUE:"));
    assert_eq!(crate::chatgpt_routing::request_id(&row.3), Some(id));
    assert_eq!(done["taskId"], "task-a");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 2);
    let permissions: (String, i64) =
        sqlx::query_as("SELECT project_folder,allow_execute FROM tasks WHERE id='task-a'")
            .fetch_one(state.repository.pool())
            .await
            .unwrap();
    assert_eq!(permissions, ("D:\\project".to_owned(), 0));
    let started = format!("/api/local/chatgpt/bridge/{id}/started");
    let bound = expect_json(extension_request(&app,"POST",&started,json!({"conversationId":"new-chat","conversationUrl":"https://chatgpt.com/g/g-p-project/c/new-chat"})).await,StatusCode::OK).await;
    assert_eq!(bound["taskId"], "task-a");
}

#[tokio::test]
async fn compact_work_resume_does_not_overtake_a_real_user_message() {
    let (state, app, _dir) = fixture().await;
    expect_json(
        extension_request(
            &app,
            "POST",
            TASK_PATH,
            json!({"continueAfterCompact":true}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let job = ready(&app).await;
    complete(&app, &job).await;
    let cookie = cookie(&state).await;
    let (status, _) = gui_post(
        &app,
        "/api/local/chatgpt/tasks/task-a/messages",
        json!({"content":"User continues explicitly"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let path = format!(
        "/api/local/chatgpt/compact/{}/resume",
        job["id"].as_str().unwrap()
    );
    let response = expect_json(
        extension_request(&app, "POST", &path, json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert!(response["requestId"].is_null());
    assert_eq!(response["alreadyContinued"], true);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chatgpt_bridge_requests WHERE id LIKE 'compact-continue-%'",
    )
    .fetch_one(state.repository.pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn compact_handoff_waits_for_every_admitted_local_call() {
    let (state, app, _dir) = fixture().await;
    let job = start(&app).await;
    let mut context = OperationContext::new("in-flight-command", "agent", "command_run");
    context.conversation_scope_id = Some(chatcmd_storage::compact::openai_scope("old-chat-a"));
    let call = state.activities.track_compact_call(&context).unwrap();
    checkpoint(
        &app,
        &job,
        json!({"phase":"writing_handoff"}),
        StatusCode::CONFLICT,
    )
    .await;
    drop(call);
    let next = checkpoint(
        &app,
        &job,
        json!({"phase":"writing_handoff"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(next["phase"], "writing_handoff");
}
