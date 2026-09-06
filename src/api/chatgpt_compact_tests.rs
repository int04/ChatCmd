use super::chatgpt_compact_test_support::*;
use super::chatgpt_router_tests::{expect_json, extension_request, gui_get};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;

#[tokio::test]
async fn compact_routes_share_exact_contract_and_idempotent_running_start() {
    let (state, app, _dir) = fixture().await;
    let job = start(&app).await;
    let duplicate = start(&app).await;
    assert_eq!(duplicate, job);
    for field in [
        "id",
        "taskId",
        "phase",
        "revision",
        "oldConversationId",
        "oldConversationUrl",
        "oldModel",
        "oldRequestId",
        "oldScopeHash",
        "newConversationId",
        "newConversationUrl",
        "handoffText",
        "detail",
        "createdAtMs",
        "updatedAtMs",
        "completedAtMs",
        "agentName",
        "projectFolder",
    ] {
        assert!(job.get(field).is_some(), "missing {field}");
    }
    assert_eq!(job["taskId"], "task-a");
    assert_eq!(job["revision"], 0);
    assert_eq!(job["phase"], "preparing");
    assert_eq!(job["oldRequestId"], "request-a");
    assert_eq!(job["oldModel"], "Pro");
    let stored: String =
        sqlx::query_scalar("SELECT status FROM chatgpt_bridge_requests WHERE id='request-a'")
            .fetch_one(state.repository.pool())
            .await
            .expect("status");
    assert_eq!(stored, "stop_requested");
    let changed = checkpoint(
        &app,
        &job,
        json!({"phase":"writing_handoff","detail":"working"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(changed["revision"], 1);
    checkpoint(
        &app,
        &job,
        json!({"phase":"writing_handoff","detail":"stale"}),
        StatusCode::CONFLICT,
    )
    .await;
    let path = format!(
        "/api/local/chatgpt/compact/{}",
        job["id"].as_str().expect("id")
    );
    let current = expect_json(
        extension_request(&app, "GET", &path, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(current["revision"], 1);
    assert_eq!(current["detail"], "working");
    let saved = checkpoint(
        &app,
        &current,
        json!({"phase":"saving_handoff","handoffText":"Private handoff"}),
        StatusCode::OK,
    )
    .await;
    let full = expect_json(
        extension_request(&app, "GET", &path, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(full["handoffText"], "Private handoff");
    let pending = expect_json(
        extension_request(
            &app,
            "GET",
            "/api/local/chatgpt/compact/pending",
            Value::Null,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(pending["jobs"].as_array().expect("jobs").len(), 1);
    assert!(pending["jobs"][0]["handoffText"].is_null());
    let task = expect_json(
        extension_request(&app, "GET", TASK_PATH, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(task["active"]["id"], saved["id"]);
    assert!(task["active"]["handoffText"].is_null());
    assert_eq!(task["history"], json!([]));
}

#[tokio::test]
async fn compact_routes_complete_existing_task_and_hide_history_handoff() {
    let (state, app, _dir) = fixture().await;
    let job = start(&app).await;
    checkpoint(&app,&job,json!({"phase":"completed","handoffText":"not saved yet","newConversationId":"new-chat","newConversationUrl":"https://chatgpt.com/c/new-chat"}),StatusCode::CONFLICT).await;
    let job = ready(&app).await;
    checkpoint(&app,&job,json!({"phase":"completed","newConversationId":"new-chat","newConversationUrl":"https://chatgpt.com/c/mismatched"}),StatusCode::BAD_REQUEST).await;
    checkpoint(&app,&job,json!({"phase":"completed","newConversationId":"old-chat-b","newConversationUrl":"https://chatgpt.com/c/old-chat-b"}),StatusCode::CONFLICT).await;
    let done = complete(&app, &job).await;
    assert_eq!(done["phase"], "completed");
    assert_eq!(done["taskId"], "task-a");
    checkpoint(
        &app,
        &job,
        json!({"phase":"completed"}),
        StatusCode::CONFLICT,
    )
    .await;
    let task = expect_json(
        extension_request(&app, "GET", TASK_PATH, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert!(task["active"].is_null());
    assert_eq!(task["history"].as_array().expect("history").len(), 1);
    assert!(task["history"][0]["handoffText"].is_null());
    let row=sqlx::query("SELECT t.title,t.project_folder,t.allow_execute,t.conversation_scope_hash,c.conversation_id FROM tasks t JOIN chatgpt_conversations c ON c.task_id=t.id WHERE t.id='task-a'")
        .fetch_one(state.repository.pool()).await.expect("task");
    assert_eq!(row.get::<String, _>("title"), "Bridge router test");
    assert_eq!(row.get::<String, _>("project_folder"), "D:\\project");
    assert_eq!(row.get::<i64, _>("allow_execute"), 0);
    assert_eq!(row.get::<String, _>("conversation_id"), "new-chat");
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        chatcmd_storage::compact::openai_scope("new-chat")
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .expect("count");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn compact_auth_keeps_extension_allowlist_and_encrypted_gui_sessions() {
    let (state, app, _dir) = fixture().await;
    for path in [TASK_PATH, "/api/local/chatgpt/compact/pending"] {
        expect_json(
            extension_request(&app, "GET", path, Value::Null).await,
            StatusCode::OK,
        )
        .await;
        let (status, _) = gui_get(&app, path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("X-ChatCmdClient", "local-ui")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for (method, path) in [
        ("GET", "/api/local/settings"),
        ("POST", "/api/local/chatgpt/requests"),
        ("POST", "/api/local/tasks/task-a/stop"),
    ] {
        assert_eq!(
            extension_request(&app, method, path, json!({}))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let cookie = cookie(&state).await;
    let (status, job) = gui_post(&app, TASK_PATH, json!({}), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(job["taskId"], "task-a");
    let (status, list) = gui_get(&app, TASK_PATH, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["active"]["id"], job["id"]);
    let (status, _) = gui_post(&app, TASK_PATH, json!({}), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn compact_server_blocks_send_but_preserves_queued_user_messages() {
    let (state, app, _dir) = fixture().await;
    let cookie = cookie(&state).await;
    // Finished requests must not be the reason this send is blocked: compact itself is the guard.
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='completed' WHERE task_id='task-a'")
        .execute(state.repository.pool())
        .await
        .expect("finish previous request");
    let job = start(&app).await;
    let (status, _) = gui_post(
        &app,
        "/api/local/chatgpt/tasks/task-a/messages",
        json!({"content":"during compact"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, queued) = gui_post(
        &app,
        "/api/local/chatgpt/tasks/task-a/queue",
        json!({"content":"keep my message","mode":"immediate"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(queued["content"], "keep my message");
    assert_eq!(queued["mode"], "queued");
    let claimed = crate::chatgpt_queue::claim_immediate(&state.repository, "task-a")
        .await
        .expect("claim immediate");
    assert!(claimed.is_empty());
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_message_queue WHERE task_id='task-a'")
            .fetch_one(state.repository.pool())
            .await
            .expect("preserved queue");
    assert_eq!(count, 1);
    checkpoint(&app, &job, json!({"phase":"cancelled"}), StatusCode::OK).await;
    let (status, sent) = gui_post(
        &app,
        "/api/local/chatgpt/tasks/task-a/messages",
        json!({"content":"after cancellation"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sent["taskId"], "task-a");
}
