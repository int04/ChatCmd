use super::chatgpt_compact_test_support::*;
use super::chatgpt_router_tests::{expect_json, extension_request};
use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn compact_default_and_explicit_off_never_create_a_working_message() {
    for input in [json!({}), json!({"continueAfterCompact":false})] {
        let (state, app, _dir) = fixture().await;
        let created = expect_json(
            extension_request(&app, "POST", TASK_PATH, input).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(created["continueAfterCompact"], false);
        // A second click/reconnect cannot silently opt in the existing job.
        let duplicate = expect_json(
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
        assert_eq!(created, duplicate);
        let job = ready(&app).await;
        let done = complete(&app, &job).await;
        assert_eq!(done["continueAfterCompact"], false);
        let path = format!(
            "/api/local/chatgpt/compact/{}/resume",
            job["id"].as_str().expect("id")
        );
        for body in [json!({}), json!({"continueAfterCompact":true})] {
            let result = expect_json(
                extension_request(&app, "POST", &path, body).await,
                StatusCode::OK,
            )
            .await;
            assert!(result["requestId"].is_null());
            assert_eq!(result["continueAfterCompact"], false);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chatgpt_bridge_requests WHERE id LIKE 'compact-continue-%'",
        )
        .fetch_one(state.repository.pool())
        .await
        .expect("count requests");
        assert_eq!(count, 0);
        let binding: String = sqlx::query_scalar(
            "SELECT conversation_id FROM chatgpt_conversations WHERE task_id='task-a'",
        )
        .fetch_one(state.repository.pool())
        .await
        .expect("same task attached");
        assert_eq!(
            binding, "new-chat",
            "opting out does not skip the handoff attachment"
        );
    }
}

#[tokio::test]
async fn compact_encrypted_gui_explicit_opt_in_is_retained_in_all_views() {
    let (state, app, _dir) = fixture().await;
    let auth = cookie(&state).await;
    let (status, job) = gui_post(
        &app,
        TASK_PATH,
        json!({"continueAfterCompact":true}),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(job["continueAfterCompact"], true);
    let summary = expect_json(
        extension_request(&app, "GET", TASK_PATH, json!(null)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(summary["active"]["continueAfterCompact"], true);
    let pending = expect_json(
        extension_request(
            &app,
            "GET",
            "/api/local/chatgpt/compact/pending",
            json!(null),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(pending["jobs"][0]["continueAfterCompact"], true);
    let persisted = ready(&app).await;
    let done = complete(&app, &persisted).await;
    assert_eq!(done["continueAfterCompact"], true);
    let history = expect_json(
        extension_request(&app, "GET", TASK_PATH, json!(null)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(history["history"][0]["continueAfterCompact"], true);
}

#[tokio::test]
async fn compact_opt_in_requires_a_json_boolean() {
    let (state, app, _dir) = fixture().await;
    for value in [json!("true"), json!(1), json!(null)] {
        let response = extension_request(
            &app,
            "POST",
            TASK_PATH,
            json!({"continueAfterCompact":value}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_compact_jobs")
        .fetch_one(state.repository.pool())
        .await
        .expect("count jobs");
    assert_eq!(count, 0);
}
