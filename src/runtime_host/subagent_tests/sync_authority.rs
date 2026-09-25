use super::*;

#[tokio::test]
async fn browser_observation_cannot_replace_child_mcp_initialization() {
    let (host, parent, registration, id, _directory) = fallback_fixture().await;
    let child = registration["childTaskId"].as_str().unwrap();
    let turn = format!("turn-{id}");
    let mut context = OperationContext::new(
        "unsynced-child-final",
        &parent.agent_id,
        "agent_turn_complete",
    );
    context.task_id = Some(child.into());
    context.turn_id = Some(turn.clone());
    assert_eq!(
        host.ensure_user_message_synced(&context)
            .await
            .unwrap_err()
            .code,
        "user_message_sync_required"
    );

    // A browser transcript is observable data, not a successful MCP initialization.
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('browser-only-user',?,?,'user','message','browser-only-user',?,?)")
        .bind(child).bind(&turn)
        .bind(json!({"provider":"chatgpt_web","content":"Read one file"}).to_string())
        .bind(now_ms()).execute(host.repository.pool()).await.unwrap();
    assert_eq!(
        host.ensure_user_message_synced(&context)
            .await
            .unwrap_err()
            .code,
        "user_message_sync_required"
    );
}

#[tokio::test]
async fn synchronized_child_can_report_blocked_work_without_reading_a_file() {
    let (host, parent, registration, id, _directory) = fallback_fixture().await;
    let mut context =
        OperationContext::new("sync-blocked-child", &parent.agent_id, "agent_user_message");
    context.task_id = registration["childTaskId"].as_str().map(str::to_owned);
    context.turn_id = Some(format!("turn-{id}"));
    let synced = host
        .call_persisted(
            "agent_user_message",
            context.clone(),
            json!({"content":delegated_prompt(&id)}),
        )
        .await
        .unwrap();
    assert_eq!(synced["userMessageSynced"], true);
    context.request_id = "finish-blocked-child".into();
    context.tool_name = "agent_turn_complete".into();
    let completed = host.call_persisted("agent_turn_complete", context,
        json!({"content":"Blocked. No files were inspected or changed.","workOutcome":"blocked","verificationIntent":"notRun"})).await.unwrap();
    assert_eq!(completed["accepted"], true);
    assert_eq!(completed["qualityReport"]["workOutcome"], "blocked");
    let report =
        chatcmd_storage::subagent_report::report_page(host.repository.pool(), &id, 0, 1000)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(report["source"], "mcpFinal");
    assert_eq!(report["workOutcome"], "blocked");
}
