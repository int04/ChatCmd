use super::chatgpt_compact_test_support::*;
use super::chatgpt_router_tests::{expect_json, extension_request};
use axum::http::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn compact_archived_callbacks_cannot_rebind_or_create_recorder_tasks() {
    let (state, app, _dir) = fixture().await;
    // Seed an unrelated unbound request before compact, to test ghost-task prevention.
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) SELECT 'unbound',NULL,'unbound-turn',agent_id,model,'other','other','queued',created_at_ms,updated_at_ms FROM chatgpt_bridge_requests WHERE id='request-b'")
        .execute(state.repository.pool()).await.expect("unbound request");
    let job = ready(&app).await;
    complete(&app, &job).await;
    let old = json!({"conversationId":"old-chat-a","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-a"});
    let callbacks = [
        ("started", old.clone()),
        ("identity", old.clone()),
        (
            "result",
            json!({"status":"completed","assistantContent":"obsolete answer","conversationId":"old-chat-a","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-a"}),
        ),
        (
            "browser-completed",
            json!({"assistantContent":"obsolete answer","conversationId":"old-chat-a","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-a"}),
        ),
        (
            "observation",
            json!({"revision":99,"messages":[],"conversationId":"old-chat-a","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-a"}),
        ),
    ];
    for (action, body) in callbacks {
        let path = format!("/api/local/chatgpt/bridge/request-a/{action}");
        expect_json(
            extension_request(&app, "POST", &path, body).await,
            StatusCode::CONFLICT,
        )
        .await;
    }
    // An obsolete request cannot claim the new identity either.
    expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/bridge/request-a/identity",
            json!({"conversationId":"new-chat","conversationUrl":"https://chatgpt.com/c/new-chat"}),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;
    expect_json(
        extension_request(
            &app,
            "POST",
            "/api/local/chatgpt/bridge/unbound/started",
            old,
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;
    let archived_native = json!({"conversationId":"old-chat-a","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-a","userMessageId":"archived-browser-message","content":"history only"});
    for _ in 0..2 {
        expect_json(
            extension_request(
                &app,
                "POST",
                "/api/local/chatgpt/capture/turns",
                archived_native.clone(),
            )
            .await,
            StatusCode::CONFLICT,
        )
        .await;
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .expect("task count");
    assert_eq!(count, 2);
    let recorder: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mcp_agents WHERE id='chatgpt-browser-recorder'")
            .fetch_one(state.repository.pool())
            .await
            .expect("recorder count");
    assert_eq!(recorder, 0);
    let current: String = sqlx::query_scalar(
        "SELECT conversation_id FROM chatgpt_conversations WHERE task_id='task-a'",
    )
    .fetch_one(state.repository.pool())
    .await
    .expect("binding");
    assert_eq!(current, "new-chat");
    let old_request = expect_json(
        extension_request(
            &app,
            "GET",
            "/api/local/chatgpt/requests/request-a",
            Value::Null,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(old_request["status"], "stopped");
    // Existing normal late identity recovery is not globally disabled.
    expect_json(extension_request(&app,"POST","/api/local/chatgpt/bridge/request-b/identity",json!({"conversationId":"old-chat-b","conversationUrl":"https://chatgpt.com/g/g-p-project/c/old-chat-b"})).await,StatusCode::OK).await;
}

#[tokio::test]
async fn compact_active_owns_native_source_and_reserved_seed_until_completion() {
    let (state, app, _dir) = fixture().await;
    let job = ready(&app).await;
    let job=checkpoint(&app,&job,json!({"newConversationId":"new-chat","newConversationUrl":"https://chatgpt.com/g/g-p-project/c/new-chat"}),StatusCode::OK).await;
    for id in ["old-chat-a", "new-chat"] {
        expect_json(extension_request(&app,"POST","/api/local/chatgpt/capture/turns",json!({
            "conversationId":id,"conversationUrl":format!("https://chatgpt.com/g/g-p-project/c/{id}"),"userMessageId":"seed-message","content":"handoff seed, no tools"
        })).await,StatusCode::CONFLICT).await;
    }
    complete(&app, &job).await;
    let capture=expect_json(extension_request(&app,"POST","/api/local/chatgpt/capture/turns",json!({
        "conversationId":"new-chat","conversationUrl":"https://chatgpt.com/g/g-p-project/c/new-chat","userMessageId":"real-followup","content":"new user followup"
    })).await,StatusCode::OK).await;
    assert_eq!(capture["taskId"], "task-a");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .expect("task count");
    assert_eq!(count, 2);
    let permission: i64 = sqlx::query_scalar("SELECT allow_execute FROM tasks WHERE id='task-a'")
        .fetch_one(state.repository.pool())
        .await
        .expect("permission");
    assert_eq!(permission, 0);
}
