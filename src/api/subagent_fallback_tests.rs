use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use tempfile::TempDir;

use super::*;
use crate::runtime_host::user_message_tests::test_host;

const SUBAGENT_ID: &str = "subagent-fallback-api";
const PARENT_TASK_ID: &str = "task-fallback-api-parent";
const CHILD_TASK_ID: &str = "task-fallback-api-child";
const PARENT_TURN_ID: &str = "turn-fallback-api-parent";

async fn fixture() -> (Arc<AppState>, TempDir) {
    let (host, agent_id, directory) = test_host().await;
    let state =
        Arc::new(host.test_app_state(directory.path().join("chatcmd.db").display().to_string()));
    let now = now_ms();
    for (task_id, title, status) in [
        (PARENT_TASK_ID, "Fallback API parent", "running"),
        (CHILD_TASK_ID, "Fallback API child", "pending"),
    ] {
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'mcp',?,1,?,?)")
            .bind(task_id)
            .bind(&agent_id)
            .bind(state.device.id.as_str())
            .bind(title)
            .bind(status)
            .bind(now)
            .bind(now)
            .execute(state.repository.pool())
            .await
            .expect("insert fallback API task");
    }
    sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms,completed_at_ms,fallback_state,fallback_attempts) VALUES(?,?,?,?,?,?,'pending',?,?,NULL,'requested',1)")
        .bind(SUBAGENT_ID)
        .bind(PARENT_TASK_ID)
        .bind(PARENT_TURN_ID)
        .bind(CHILD_TASK_ID)
        .bind("Fallback API child")
        .bind("Inspect delegated source")
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .expect("insert fallback run");
    (state, directory)
}

#[tokio::test]
async fn pending_api_returns_queued_child_with_marker_and_parent_identity() {
    let (state, _directory) = fixture().await;
    let Json(pending) = pending_subagent_fallbacks(State(state))
        .await
        .expect("pending fallback API");
    assert_eq!(pending.len(), 1);
    let item = &pending[0];
    assert_eq!(item["subagentId"], SUBAGENT_ID);
    assert_eq!(item["childTaskId"], CHILD_TASK_ID);
    assert_eq!(item["parentTaskId"], PARENT_TASK_ID);
    assert_eq!(item["parentTurnId"], PARENT_TURN_ID);
    assert_eq!(item["attempt"], 1);
    assert!(
        item["submittedContent"]
            .as_str()
            .is_some_and(|value| value.contains("CMDGPT_SUBAGENT_ID=subagent-fallback-api"))
    );
}

#[tokio::test]
async fn started_callback_persists_conversation_identity_for_child_task() {
    let (state, _directory) = fixture().await;
    let conversation_id = "conversation-fallback-started";
    let conversation_url = "https://chatgpt.com/c/conversation-fallback-started";
    let Json(result) = subagent_fallback_started(
        State(state.clone()),
        Path(SUBAGENT_ID.to_owned()),
        Json(SubagentFallbackStarted {
            attempt: 1,
            conversation_id: Some(conversation_id.to_owned()),
            conversation_url: Some(conversation_url.to_owned()),
        }),
    )
    .await
    .expect("started callback");
    assert_eq!(result["accepted"], true);

    let run = sqlx::query("SELECT fallback_state,fallback_conversation_id,fallback_conversation_url FROM subagent_runs WHERE id=?")
        .bind(SUBAGENT_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read started run");
    assert_eq!(run.get::<String, _>("fallback_state"), "started");
    assert_eq!(
        run.get::<Option<String>, _>("fallback_conversation_id")
            .as_deref(),
        Some(conversation_id)
    );
    let linked_task: String =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_conversations WHERE conversation_id=?")
            .bind(conversation_id)
            .fetch_one(state.repository.pool())
            .await
            .expect("conversation identity link");
    assert_eq!(linked_task, CHILD_TASK_ID);
}

#[tokio::test]
async fn stale_browser_result_after_mcp_claim_is_ignored_without_retry() {
    let (state, _directory) = fixture().await;
    sqlx::query("UPDATE subagent_runs SET status='running',fallback_state='claimed' WHERE id=?")
        .bind(SUBAGENT_ID)
        .execute(state.repository.pool())
        .await
        .expect("claim fallback");

    let Json(result) = subagent_fallback_result(
        State(state.clone()),
        Path(SUBAGENT_ID.to_owned()),
        Json(SubagentFallbackResult {
            attempt: 1,
            status: "failed".to_owned(),
            assistant_content: None,
            error_message: Some("stale browser failure".to_owned()),
            conversation_id: None,
            conversation_url: None,
        }),
    )
    .await
    .expect("stale result");
    assert_eq!(result["accepted"], false);
    assert_eq!(result["reason"], "already_claimed_or_finished");

    let run =
        sqlx::query("SELECT status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=?")
            .bind(SUBAGENT_ID)
            .fetch_one(state.repository.pool())
            .await
            .expect("read claimed run");
    assert_eq!(run.get::<String, _>("status"), "running");
    assert_eq!(run.get::<String, _>("fallback_state"), "claimed");
    assert_eq!(run.get::<i64, _>("fallback_attempts"), 1);
}

#[tokio::test]
async fn browser_failures_retry_same_child_then_exhaust_on_attempt_three() {
    let (state, _directory) = fixture().await;
    for attempt in 1..=3 {
        let Json(result) = subagent_fallback_result(
            State(state.clone()),
            Path(SUBAGENT_ID.to_owned()),
            Json(SubagentFallbackResult {
                attempt,
                status: "failed".to_owned(),
                assistant_content: None,
                error_message: Some(format!("attempt {attempt} failed")),
                conversation_id: None,
                conversation_url: None,
            }),
        )
        .await
        .expect("fallback failure result");
        if attempt < 3 {
            assert_eq!(result["retryScheduled"], true);
            assert_eq!(result["attempt"], attempt + 1);
            let run = sqlx::query("SELECT child_task_id,status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=?")
                .bind(SUBAGENT_ID)
                .fetch_one(state.repository.pool())
                .await
                .expect("read retry run");
            assert_eq!(
                run.get::<Option<String>, _>("child_task_id").as_deref(),
                Some(CHILD_TASK_ID)
            );
            assert_eq!(run.get::<String, _>("status"), "pending");
            assert_eq!(run.get::<String, _>("fallback_state"), "requested");
            assert_eq!(run.get::<i64, _>("fallback_attempts"), attempt + 1);
        } else {
            assert_eq!(result["exhausted"], true);
            assert_eq!(result["retryScheduled"], false);
        }
    }

    let run = sqlx::query("SELECT child_task_id,status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=?")
        .bind(SUBAGENT_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read exhausted run");
    assert_eq!(
        run.get::<Option<String>, _>("child_task_id").as_deref(),
        Some(CHILD_TASK_ID)
    );
    assert_eq!(run.get::<String, _>("status"), "failed");
    assert_eq!(run.get::<String, _>("fallback_state"), "exhausted");
    assert_eq!(run.get::<i64, _>("fallback_attempts"), 3);
    let task_status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read failed child task");
    assert_eq!(task_status, "failed");
}

#[tokio::test]
async fn browser_only_final_response_completes_child_and_saves_conversation() {
    let (state, _directory) = fixture().await;
    let conversation_id = "conversation-browser-only-child";
    let conversation_url = "https://chatgpt.com/c/conversation-browser-only-child";
    let Json(result) = subagent_fallback_result(
        State(state.clone()),
        Path(SUBAGENT_ID.to_owned()),
        Json(SubagentFallbackResult {
            attempt: 1,
            status: "completed".to_owned(),
            assistant_content: Some("Browser-only delegated answer".to_owned()),
            error_message: None,
            conversation_id: Some(conversation_id.to_owned()),
            conversation_url: Some(conversation_url.to_owned()),
        }),
    )
    .await
    .expect("browser-only completion");
    assert_eq!(result["accepted"], true);
    assert_eq!(result["completed"], true);
    assert_eq!(result["retryScheduled"], false);

    let run_status: String = sqlx::query_scalar("SELECT status FROM subagent_runs WHERE id=?")
        .bind(SUBAGENT_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read completed run");
    assert_eq!(run_status, "completed");
    let task_status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=?")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read completed child task");
    assert_eq!(task_status, "completed");
    let linked_task: String =
        sqlx::query_scalar("SELECT task_id FROM chatgpt_conversations WHERE conversation_id=?")
            .bind(conversation_id)
            .fetch_one(state.repository.pool())
            .await
            .expect("read browser-only conversation link");
    assert_eq!(linked_task, CHILD_TASK_ID);
    let final_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.status')='completed'")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("count browser-only final events");
    assert_eq!(final_count, 1);
    let stored_answer: String = sqlx::query_scalar("SELECT json_extract(payload_json,'$.content') FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.status')='completed' LIMIT 1")
        .bind(CHILD_TASK_ID)
        .fetch_one(state.repository.pool())
        .await
        .expect("read browser-only final answer");
    assert_eq!(stored_answer, "Browser-only delegated answer");
}
