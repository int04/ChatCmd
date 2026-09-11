//! Request identity must not depend on the model copying every word correctly.
use super::{RuntimeHost, now_ms};
use crate::runtime_host::user_message_tests::{test_host, turn_context};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) const TASK: &str = "task-chat-7b4abf70-f6ce-5cd2-a0a0-3d22ce3c45fd";
pub(super) const REQUEST: &str = "7501be15-5efe-4a00-8f98-4738f636e320";
pub(super) const TURN: &str = "chatgpt-turn-36c9f260-7305-4e59-8acb-c9866930f99c";
pub(super) const SCOPE: &str = "openai:mcp-scope-not-browser-id";

pub(super) fn fixture() -> (String, String) {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/chatui_paraphrased_gradle.json")).unwrap();
    (
        data["submitted"].as_str().unwrap().to_owned(),
        data["echo"].as_str().unwrap().to_owned(),
    )
}

pub(super) async fn seed(host: &RuntimeHost, agent: &str, submitted: &str) {
    let now = now_ms();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,'openai:browser-scope','chatgpt_web',1,'running',1,?,?)")
        .bind(TASK).bind(agent).bind(host.device.id.as_str()).bind(now).bind(now)
        .execute(host.repository.pool()).await.unwrap();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'Auto',?,?,'running',?,?)")
        .bind(REQUEST).bind(TASK).bind(TURN).bind(agent).bind(submitted).bind(submitted)
        .bind(now).bind(now).execute(host.repository.pool()).await.unwrap();
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,'browser-id','https://chatgpt.com/c/browser-id','Auto',?,?,?)")
        .bind(TASK).bind(REQUEST).bind(now).bind(now).execute(host.repository.pool()).await.unwrap();
}

pub(super) async fn count(host: &RuntimeHost) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(host.repository.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn legacy_paraphrase_fails_without_approval_or_ghost_and_can_recover_by_server_turn() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    sqlx::query("UPDATE settings SET value_json='true' WHERE key='ui_approveNewConversations'")
        .execute(host.repository.pool())
        .await
        .unwrap();
    let mut context = turn_context(
        "legacy-drift",
        &agent,
        "agent_user_message",
        "model-generated-turn",
        SCOPE,
    );
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        host.ensure_call_identity(&mut context, Some(&echo)),
    )
    .await;
    assert_eq!(
        count(&host).await,
        1,
        "a paraphrased echo must never create a second task"
    );
    let error = result
        .expect("no approval wait")
        .expect_err("request identity required");
    assert_eq!(error.code, "conversation_identity_required");
    assert!(
        error.message.contains(TURN),
        "recovery must expose the server-owned turn ID"
    );
    let mut retry = turn_context("legacy-retry", &agent, "agent_user_message", TURN, SCOPE);
    tokio::time::timeout(
        Duration::from_secs(2),
        host.ensure_call_identity(&mut retry, Some(&echo)),
    )
    .await
    .expect("retry must not wait for approval")
    .expect("route recovery");
    assert_eq!(retry.task_id.as_deref(), Some(TASK));
    assert_eq!(count(&host).await, 1);
}

#[tokio::test]
async fn server_turn_routes_rewritten_text_and_links_transcript() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, echo) = fixture();
    seed(&host, &agent, &submitted).await;
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context("route-by-turn", &agent, "agent_user_message", TURN, SCOPE),
            json!({"content": echo}),
        )
        .await
        .expect("route by recorded turn");
    assert_eq!(accepted["taskId"], TASK);
    assert_eq!(count(&host).await, 1);
    let payload: String = sqlx::query_scalar("SELECT payload_json FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='user' AND kind='message'")
        .bind(TASK).bind(TURN).fetch_one(host.repository.pool()).await.unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["bridgeRequestId"], REQUEST);
    assert_eq!(payload["browserTurnId"], TURN);
}

#[tokio::test]
async fn mcp_scope_stays_bound_after_request_completes_and_a_new_turn_arrives() {
    let (host, agent, _dir) = test_host().await;
    let (submitted, _) = fixture();
    seed(&host, &agent, &submitted).await;
    let mut first = turn_context(
        "first-echo",
        &agent,
        "agent_user_message",
        "first-turn",
        SCOPE,
    );
    host.ensure_call_identity(&mut first, Some(&submitted))
        .await
        .unwrap();
    assert_eq!(first.task_id.as_deref(), Some(TASK));
    sqlx::query(
        "UPDATE chatgpt_bridge_requests SET status='completed',completed_at_ms=? WHERE id=?",
    )
    .bind(now_ms())
    .bind(REQUEST)
    .execute(host.repository.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL WHERE task_id=?")
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='completed' WHERE id=?")
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    let mut next = turn_context("follow-up", &agent, "agent_user_message", "new-turn", SCOPE);
    host.ensure_call_identity(&mut next, Some("commit"))
        .await
        .unwrap();
    assert_eq!(next.task_id.as_deref(), Some(TASK));
    assert_eq!(count(&host).await, 1);
}
