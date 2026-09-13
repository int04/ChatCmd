//! The local send handlers and real MCP runtime must agree without exact text echo.
use super::*;
use crate::runtime_host::user_message_tests::test_host;
use chatcmd_mcp::RuntimeApi as _;
use chatcmd_runtime::OperationContext;

#[tokio::test]
async fn chatui_send_mcp_first_browser_callback_and_followup_keep_one_task() {
    let (host, agent, dir) = test_host().await;
    let state = Arc::new(host.test_app_state(dir.path().join("chatcmd.db").display().to_string()));
    sqlx::query("UPDATE settings SET value_json='true' WHERE key='ui_approveNewConversations'")
        .execute(state.repository.pool())
        .await
        .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../runtime_host/fixtures/chatui_paraphrased_gradle.json"
    ))
    .unwrap();
    let content = fixture["submitted"].as_str().unwrap();
    let created = create_request(
        State(state.clone()),
        Json(CreateRequest {
            agent_id: agent.clone(),
            model: None,
            project_folder: Some(dir.path().display().to_string()),
            content: content.to_owned(),
        }),
    )
    .await
    .unwrap()
    .0;
    let request = created["id"].as_str().unwrap();
    let turn = created["turnId"].as_str().unwrap();
    assert!(created["taskId"].is_null());
    assert_eq!(created["userContent"], content);
    let submitted = created["submittedContent"].as_str().unwrap();
    assert_eq!(crate::chatgpt_routing::request_id(submitted), Some(request));
    assert!(submitted.contains("api_tool.list_resources on the current connector"));
    assert!(submitted.contains("query \"agent_user_message\" (fallback \"agent\")"));
    assert!(submitted.contains("call agent_user_message in this same turn before replying"));
    assert!(submitted.contains("not attempted is not blocked"));
    let mut context = OperationContext::new("early-mcp", &agent, "agent_user_message");
    context.turn_id = Some(turn.to_owned());
    context.conversation_scope_id = Some("openai:provider-session".to_owned());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        host.call(
            "agent_user_message",
            context,
            json!({"content":fixture["echo"]}),
        ),
    )
    .await
    .expect("no fresh approval wait")
    .expect("route MCP before browser callback");
    let task = result["taskId"].as_str().unwrap();
    let started = bridge_started(
        State(state.clone()),
        Path(request.to_owned()),
        Json(BridgeStarted {
            conversation_id: "browser-conversation".to_owned(),
            conversation_url: "https://chatgpt.com/c/browser-conversation".to_owned(),
            model: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(started["taskId"], task);
    sqlx::query(
        "UPDATE chatgpt_bridge_requests SET status='completed',completed_at_ms=? WHERE id=?",
    )
    .bind(now_ms())
    .bind(request)
    .execute(state.repository.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL WHERE task_id=?")
        .bind(task)
        .execute(state.repository.pool())
        .await
        .unwrap();
    let followup = continue_message(
        State(state.clone()),
        Path(task.to_owned()),
        Json(ContinueRequest {
            model: None,
            content: "commit".to_owned(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_ne!(followup["turnId"], turn);
    assert_eq!(followup["userContent"], "commit");
    let mut next = OperationContext::new("followup-mcp", &agent, "agent_user_message");
    next.turn_id = Some(followup["turnId"].as_str().unwrap().to_owned());
    next.conversation_scope_id = Some("openai:rotated-provider-session".to_owned());
    let accepted = host
        .call("agent_user_message", next, json!({"content":"commit"}))
        .await
        .unwrap();
    assert_eq!(accepted["taskId"], task);
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE allow_execute IS NULL")
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(tasks, 1);
    assert_eq!(pending, 0);
    let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE actor='user' AND kind='message' AND json_extract(payload_json,'$.bridgeRequestId') IS NOT NULL")
        .fetch_one(state.repository.pool()).await.unwrap();
    assert_eq!(linked, 2);
}

#[tokio::test]
async fn recorder_send_does_not_inject_mcp_routing_instructions() {
    let (host, agent, dir) = test_host().await;
    let state = Arc::new(host.test_app_state(dir.path().join("chatcmd.db").display().to_string()));
    sqlx::query("UPDATE mcp_agents SET name=? WHERE id=?")
        .bind(crate::api::chatgpt_native::RECORDER_AGENT_NAME)
        .bind(&agent)
        .execute(state.repository.pool())
        .await
        .unwrap();
    let created = create_request(
        State(state),
        Json(CreateRequest {
            agent_id: agent,
            model: None,
            project_folder: None,
            content: "A normal conversation".to_owned(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(created["submittedContent"], "A normal conversation");
}
