use super::*;

#[tokio::test]
async fn subagent_without_user_sync_cannot_finalize_or_read_files() {
    let (host, parent, registration, subagent_id, directory) = fallback_fixture().await;
    let child = registration["childTaskId"].as_str().unwrap();
    let turn = format!("turn-{subagent_id}");
    let file = directory.path().join("pubspec.yaml");
    std::fs::write(&file, "name: harmless_fixture\n").unwrap();
    for (tool, args) in [
        (
            "agent_turn_complete",
            json!({"content":"Initialization unavailable", "workOutcome":"blocked"}),
        ),
        ("fs_read_text", json!({"path":file})),
    ] {
        let mut context = OperationContext::new(format!("unsynced-{tool}"), &parent.agent_id, tool);
        context.task_id = Some(child.to_owned());
        context.turn_id = Some(turn.clone());
        let error = host.call_persisted(tool, context, args).await.unwrap_err();
        assert_eq!(error.code, "user_message_sync_required", "{tool}");
    }
    let final_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND actor='assistant' AND json_extract(payload_json,'$.status')='completed'")
        .bind(child).fetch_one(host.repository.pool()).await.unwrap();
    assert_eq!(
        final_count, 0,
        "an uninitialized child must not fabricate MCP completion"
    );
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "name: harmless_fixture\n"
    );
}

#[tokio::test]
async fn subagent_mcp_report_exposes_real_sync_and_finalizer_receipts() {
    let (host, parent, registration, subagent_id, _directory) = fallback_fixture().await;
    let child = registration["childTaskId"].as_str().unwrap();
    let mut context =
        OperationContext::new("synced-child-start", &parent.agent_id, "agent_user_message");
    context.task_id = Some(child.to_owned());
    context.turn_id = Some(format!("turn-{subagent_id}"));
    host.call_persisted(
        "agent_user_message",
        context.clone(),
        json!({"content":delegated_prompt(&subagent_id)}),
    )
    .await
    .unwrap();
    context.request_id = "synced-child-complete".into();
    context.tool_name = "agent_turn_complete".into();
    host.call_persisted(
        "agent_turn_complete",
        context,
        json!({"content":"Fixture report", "workOutcome":"completed"}),
    )
    .await
    .unwrap();
    let report = chatcmd_storage::subagent_report::report_page(
        host.repository.pool(),
        &subagent_id,
        0,
        12_000,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report["source"], "mcpFinal");
    assert_eq!(report["mcpUserMessageSynced"], true);
    assert_eq!(report["mcpFinalizerReceived"], true);
}
