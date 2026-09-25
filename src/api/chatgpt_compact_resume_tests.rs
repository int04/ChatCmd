use super::chatgpt_compact_test_support::*;
use super::chatgpt_router_tests::{expect_json, extension_request};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::Row;

#[tokio::test]
async fn compact_resumed_bridge_started_preserves_permissions_project_and_task() {
    let (state, app, _dir) = fixture().await;
    let job = ready(&app).await;
    complete(&app, &job).await;
    let cookie = cookie(&state).await;
    let (status, sent) = gui_post(
        &app,
        "/api/local/chatgpt/tasks/task-a/messages",
        json!({"content":"Resume ordinary work"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sent["taskId"], "task-a");
    assert_eq!(sent["userContent"], "Resume ordinary work");
    let submitted = sent["submittedContent"]
        .as_str()
        .expect("submitted content");
    assert_eq!(
        crate::chatgpt_routing::body_without_route(submitted),
        "Resume ordinary work"
    );
    assert_eq!(
        crate::chatgpt_routing::request_id(submitted),
        sent["id"].as_str()
    );
    let id = sent["id"].as_str().expect("new request id");
    // Simulate a delayed request snapshot after the user updates the task project.
    sqlx::query("UPDATE chatgpt_bridge_requests SET project_folder='D:\\stale-project' WHERE id=?")
        .bind(id)
        .execute(state.repository.pool())
        .await
        .expect("stale request project");
    let path = format!("/api/local/chatgpt/bridge/{id}/started");
    let started=expect_json(extension_request(&app,"POST",&path,json!({
        "conversationId":"new-chat","conversationUrl":"https://chatgpt.com/g/g-p-project/c/new-chat"
    })).await,StatusCode::OK).await;
    assert_eq!(started["taskId"], "task-a");
    let row = sqlx::query(
        "SELECT title,project_folder,allow_execute,generation FROM tasks WHERE id='task-a'",
    )
    .fetch_one(state.repository.pool())
    .await
    .expect("task");
    assert_eq!(row.get::<String, _>("title"), "Bridge router test");
    assert_eq!(row.get::<String, _>("project_folder"), "D:\\project");
    assert_eq!(row.get::<i64, _>("allow_execute"), 0);
    assert_eq!(row.get::<i64, _>("generation"), 2);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .expect("same task count");
    assert_eq!(count, 2);
}
