use super::*;

#[tokio::test]
async fn browser_child_marker_only_sync_resolves_registered_task_and_parent_skill_context() {
    let (host, parent, registration, subagent_id, _directory) = fallback_fixture().await;
    let child_task_id = registration["childTaskId"].as_str().unwrap();
    let turn_id = format!("turn-{subagent_id}");
    let mut context = OperationContext::new(
        "marker-only-child-sync",
        &parent.agent_id,
        "agent_user_message",
    );
    context.turn_id = Some(turn_id.clone());

    let synced = host
        .call_persisted(
            "agent_user_message",
            context,
            json!({"content":format!("CMDGPT_SUBAGENT_ID={subagent_id}")}),
        )
        .await
        .expect("the registered marker should resolve and claim its child task");

    assert_eq!(synced["taskId"], child_task_id);
    assert_eq!(synced["turnId"], turn_id);
    assert_eq!(synced["taskRole"], "delegatedChild");
    assert_eq!(synced["skillDiscovery"]["mode"], "parentOwned");
    assert_eq!(
        synced["skillDiscovery"]["requirementMode"],
        "delegatedContextFirst"
    );
    assert_eq!(synced["skillDiscovery"]["requiredForThisTask"], false);
    assert_eq!(synced["skillDiscovery"]["fallbackDiscoveryAllowed"], true);
    let skill_calls: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.tool')='skills_list'",
    )
    .bind(child_task_id)
    .fetch_one(host.repository.pool())
    .await
    .unwrap();
    assert_eq!(skill_calls, 0);
}

#[tokio::test]
async fn marker_only_child_inherits_parent_user_absolute_path_scope() {
    let (host, parent, workspace) = parent_fixture().await;
    let external = tempfile::TempDir::new().expect("external directory");
    let file = external.path().join("delegated.txt");
    std::fs::write(&file, "delegated content").expect("write external fixture");
    sqlx::query("UPDATE tasks SET project_folder=? WHERE id=?")
        .bind(workspace.path().display().to_string())
        .bind(PARENT_TASK_ID)
        .execute(host.repository.pool())
        .await
        .expect("bind parent project folder");
    sqlx::query(
        "UPDATE timeline_events SET payload_json=? WHERE event_id='event-subagent-parent-user'",
    )
    .bind(
        json!({
            "role": "user",
            "content": format!("Split the work and read exactly {}", file.display())
        })
        .to_string(),
    )
    .execute(host.repository.pool())
    .await
    .expect("give the parent task an explicit user path");
    let registration = host
        .register_subagent(
            &parent,
            "External reader",
            &format!("Read exactly {}", file.display()),
            None,
        )
        .await
        .expect("register external reader");
    let subagent_id = registration["subagentId"].as_str().unwrap();
    let child_task_id = registration["childTaskId"].as_str().unwrap();
    let turn_id = format!("turn-{subagent_id}");
    let mut sync_context = OperationContext::new(
        "external-marker-only-sync",
        &parent.agent_id,
        "agent_user_message",
    );
    sync_context.turn_id = Some(turn_id.clone());
    host.call_persisted(
        "agent_user_message",
        sync_context,
        json!({"content":format!("CMDGPT_SUBAGENT_ID={subagent_id}")}),
    )
    .await
    .expect("sync marker-only external reader");

    let mut read_context = OperationContext::new(
        "external-marker-only-read",
        &parent.agent_id,
        "fs_read_text",
    );
    read_context.task_id = Some(child_task_id.to_owned());
    read_context.turn_id = Some(turn_id);
    let scopes = host
        .task_user_path_scopes(&read_context)
        .await
        .expect("resolve delegated scopes");
    assert!(
        scopes.contains(&file.canonicalize().expect("canonical delegated file")),
        "marker-only child must inherit the explicit path from its parent user message"
    );
}

#[tokio::test]
async fn delegated_request_alone_does_not_mint_an_absolute_path_scope() {
    let (host, parent, _workspace) = parent_fixture().await;
    let external = tempfile::TempDir::new().expect("external directory");
    let file = external.path().join("model-selected.txt");
    std::fs::write(&file, "not user scoped").expect("write external fixture");
    let registration = host
        .call_persisted(
            "agent_subagent_start",
            parent.clone(),
            json!({
                "name": "Unscoped reader",
                "request": format!("Read exactly {}", file.display()),
                "allowedFiles": [file.clone()],
                "allowedEffects": ["read"]
            }),
        )
        .await
        .expect("register unscoped reader with structured allowedFiles");
    let subagent_id = registration["subagentId"].as_str().unwrap();
    let child_task_id = registration["childTaskId"].as_str().unwrap();
    let turn_id = format!("turn-{subagent_id}");
    let mut sync_context = OperationContext::new(
        "unscoped-marker-only-sync",
        &parent.agent_id,
        "agent_user_message",
    );
    sync_context.turn_id = Some(turn_id.clone());
    host.call_persisted(
        "agent_user_message",
        sync_context,
        json!({"content":format!("CMDGPT_SUBAGENT_ID={subagent_id}")}),
    )
    .await
    .expect("sync marker-only reader");

    let mut read_context = OperationContext::new(
        "unscoped-marker-only-read",
        &parent.agent_id,
        "fs_read_text",
    );
    read_context.task_id = Some(child_task_id.to_owned());
    read_context.turn_id = Some(turn_id);
    let scopes = host
        .task_user_path_scopes(&read_context)
        .await
        .expect("resolve delegated scopes");
    assert!(
        !scopes.contains(&file.canonicalize().expect("canonical delegated file")),
        "model-authored delegated text must not create a user path grant"
    );
}

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
