use super::*;
use crate::runtime_host::user_message_tests::test_host;
use serde_json::json;

#[tokio::test]
async fn subagent_original_request_lookup_is_fenced_by_task_turn_and_approval() {
    let (host, agent, _directory) = test_host().await;
    let repo = &host.repository;
    let now = crate::runtime_host::now_ms();
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES('original-task',?,?,'chatgpt_web',1,'running',1,?,?)")
        .bind(&agent).bind(host.device.id.as_str()).bind(now).bind(now)
        .execute(repo.pool()).await.unwrap();
    let submitted =
        crate::chatgpt_routing::with_route("Chia ra agent đọc file", "owned-request", "owned-turn");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES('owned-request','original-task','owned-turn',?,'Auto','Chia ra agent đọc file',?,'running',?,?)")
        .bind(&agent).bind(&submitted).bind(now).bind(now).execute(repo.pool()).await.unwrap();
    let payload = json!({"tool":"agent_user_message","content":"Chia agent theo echo",
        "bridgeRequestId":"owned-request","browserTurnId":"owned-turn"});
    let resolve = |task: &str, value: &Value| {
        let task = task.to_owned();
        let raw = value.to_string();
        async move { content(repo, &task, Some(&raw)).await.unwrap() }
    };
    assert_eq!(
        resolve("original-task", &payload).await,
        "Chia ra agent đọc file"
    );
    assert!(resolve("another-task", &payload).await.is_empty());
    let mut wrong_turn = payload.clone();
    wrong_turn["browserTurnId"] = json!("another-turn");
    assert!(resolve("original-task", &wrong_turn).await.is_empty());
    let mut missing = payload.clone();
    missing["bridgeRequestId"] = json!("missing-request");
    assert!(resolve("original-task", &missing).await.is_empty());
    let mut observed = payload.clone();
    observed["provider"] = json!("chatgpt_web");
    assert!(resolve("original-task", &observed).await.is_empty());
    sqlx::query("UPDATE tasks SET allow_execute=0 WHERE id='original-task'")
        .execute(repo.pool())
        .await
        .unwrap();
    assert!(resolve("original-task", &payload).await.is_empty());
}

#[tokio::test]
async fn subagent_plain_mcp_request_does_not_borrow_a_browser_instruction() {
    let (host, _agent, _directory) = test_host().await;
    let payload =
        json!({"tool":"agent_user_message","content":"Read without subagents"}).to_string();
    assert_eq!(
        content(&host.repository, "task", Some(&payload))
            .await
            .unwrap(),
        "Read without subagents"
    );
    assert!(
        content(&host.repository, "task", None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        content(&host.repository, "task", Some("invalid-json"))
            .await
            .unwrap()
            .is_empty()
    );
}
