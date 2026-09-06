use super::*;

#[path = "grant_bootstrap_nested.rs"]
mod nested;

fn sync_child<'a>(
    host: &'a RuntimeHost,
    parent: &'a OperationContext,
    registration: &'a Value,
) -> chatcmd_runtime::BoxFuture<'a, (OperationContext, Value)> {
    Box::pin(async move {
        let id = registration["subagentId"].as_str().expect("subagent id");
        let mut context = OperationContext::new(
            format!("bootstrap-user-{id}"),
            &parent.agent_id,
            "agent_user_message",
        );
        context.task_id = registration["childTaskId"].as_str().map(str::to_owned);
        context.turn_id = Some(format!("turn-{id}"));
        let result = Box::pin(host.call_persisted(
            "agent_user_message",
            context.clone(),
            json!({"content": format!("Read delegated files\nCMDGPT_SUBAGENT_ID={id}")}),
        ))
        .await
        .expect("child must synchronize its turn independently of optional grant inheritance");
        context.mcp_session_id = result["sessionId"].as_str().map(str::to_owned);
        (context, result)
    })
}

#[tokio::test]
async fn invalid_grant_tools_are_rejected_before_a_child_is_created() {
    let (host, context, directory) = parent_fixture().await;
    for tools in [
        vec!["git_status"],
        vec!["git_diff"],
        vec!["command_run"],
        vec!["agent_user_message"],
        vec!["agent_subagent_start"],
        vec!["fs_write_text"],
        vec!["not_a_tool"],
        vec!["fs_read_text", "fs_read_text"],
    ] {
        let mut request = read_grant_request(directory.path());
        request.allowed_tools = tools.iter().map(|name| (*name).to_owned()).collect();
        let result = host
            .register_subagent(&context, "Bad grant", "Read", Some(&request))
            .await;
        let error = result.expect_err("invalid grant must fail in the parent, before dispatch");
        assert_eq!(error.code, "approval_grant_inheritance_denied");
        assert!(
            error.message.contains(tools[0]),
            "diagnostic must identify the invalid tool"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn legacy_invalid_optional_grant_keeps_turn_sync_but_does_not_authorize_reads() {
    let (host, parent, registration, id, directory) = fallback_fixture().await;
    let mut grant = read_grant_request(directory.path());
    grant.allowed_tools = vec!["git_status".into(), "agent_subagent_start".into()];
    // Simulate an already persisted request from a client predating parent-side validation.
    sqlx::query("UPDATE subagent_runs SET requested_approval_grant_json=? WHERE id=?")
        .bind(serde_json::to_string(&grant).unwrap())
        .bind(&id)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,'approval',0)",
    )
    .bind(PARENT_TASK_ID)
    .execute(host.repository.pool())
    .await
    .unwrap();
    let (child, result) = sync_child(&host, &parent, &registration).await;
    assert_eq!(result["userMessageSynced"], true);
    assert_eq!(result["subagentApproval"]["status"], "notInherited");
    assert_eq!(
        result["subagentApproval"]["errorCode"],
        "approval_grant_inheritance_denied"
    );
    let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_grants WHERE task_id=?")
        .bind(child.task_id.as_deref())
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(grants, 0, "invalid request cannot mint a grant");
    let path = directory.path().join("protected.txt");
    std::fs::write(&path, "must still require approval").unwrap();
    let mut read = child.clone();
    read.request_id = "bootstrap-protected-read".into();
    read.tool_name = "fs_read_text".into();
    let error = Box::pin(host.call_persisted(
        "fs_read_text",
        read,
        json!({"path":path,"maxCharacters":40}),
    ))
    .await
    .expect_err("read needs user approval");
    assert_eq!(error.code, "approval_timeout");
    let wait = host.wait_for_subagents(&parent, 250).await.unwrap();
    assert_eq!(
        wait["subagents"][0]["approvalGrant"]["status"],
        "notInherited"
    );
    assert_eq!(wait["subagents"][0]["status"], "running");
}
