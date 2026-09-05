use super::*;

#[tokio::test]
async fn inherited_read_grant_is_bounded_and_reserved_from_parent() {
    let (host, context, directory) = parent_fixture().await;
    let child_scope = directory.path().join("src");
    std::fs::create_dir_all(&child_scope).expect("child scope");
    let parent_id = insert_parent_read_grant(&host, &context, directory.path(), 5, 10, 8192).await;
    let grant = read_grant_request(&child_scope);
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Inherited reader", &grant).await;
    let child_task_id = claim_registered(&host, &context, &registration, &subagent_id).await;

    let child = sqlx::query("SELECT inherited_from,child_attempt,allowed_tools_json,path_scopes_json,max_calls,max_files_scanned,max_bytes_read,state FROM approval_grants WHERE task_id=? AND inherited_from=?")
        .bind(&child_task_id)
        .bind(&parent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("child grant");
    assert_eq!(child.get::<String, _>("state"), "active");
    assert_eq!(
        child.get::<Option<String>, _>("inherited_from").as_deref(),
        Some(parent_id.as_str())
    );
    assert_eq!(child.get::<Option<i64>, _>("child_attempt"), Some(1));
    assert_eq!(child.get::<i64, _>("max_calls"), 2);
    assert_eq!(child.get::<Option<i64>, _>("max_files_scanned"), Some(4));
    assert_eq!(child.get::<Option<i64>, _>("max_bytes_read"), Some(4096));
    let tools: Value =
        serde_json::from_str(&child.get::<String, _>("allowed_tools_json")).expect("child tools");
    assert_eq!(tools, json!(["fs_read_text", "fs_stat"]));

    let parent = sqlx::query(
        "SELECT used_calls,used_files_scanned,used_bytes_read FROM approval_grants WHERE id=?",
    )
    .bind(&parent_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("parent grant budget");
    assert_eq!(parent.get::<i64, _>("used_calls"), 2);
    assert_eq!(parent.get::<i64, _>("used_files_scanned"), 4);
    assert_eq!(parent.get::<i64, _>("used_bytes_read"), 4096);

    host.finish_subagent_for_child(&child_task_id, "completed")
        .await
        .expect("finish child");
    let state: String = sqlx::query_scalar(
        "SELECT state FROM approval_grants WHERE task_id=? AND inherited_from=?",
    )
    .bind(&child_task_id)
    .bind(&parent_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("revoked child grant");
    assert_eq!(state, "revoked");
}

#[tokio::test]
async fn inherited_grant_rejects_tool_path_and_budget_escalation() {
    let (host, context, directory) = parent_fixture().await;
    let parent_scope = directory.path().join("src");
    std::fs::create_dir_all(&parent_scope).expect("parent scope");
    insert_parent_read_grant(&host, &context, &parent_scope, 2, 4, 4096).await;

    let mut tool_escalation = read_grant_request(&parent_scope);
    tool_escalation.allowed_tools = vec!["fs_write_text".to_owned()];
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Tool escalation", &tool_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-tool-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("tool escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
    host.finish_subagent_for_child(child_task_id, "failed")
        .await
        .expect("finish denied tool child");

    let mut path_escalation = read_grant_request(directory.path());
    path_escalation.max_calls = 1;
    path_escalation.max_files_scanned = 1;
    path_escalation.max_bytes_read = 1024;
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Path escalation", &path_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-path-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("path escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
    host.finish_subagent_for_child(child_task_id, "failed")
        .await
        .expect("finish denied path child");

    let mut budget_escalation = read_grant_request(&parent_scope);
    budget_escalation.max_calls = 3;
    let (registration, subagent_id) =
        register_with_grant(&host, &context, "Budget escalation", &budget_escalation).await;
    let child_task_id = registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("child task");
    let child_context = OperationContext::new(
        "claim-budget-escalation",
        &context.agent_id,
        "agent_user_message",
    );
    let error = host
        .claim_subagent_from_message(
            &child_context,
            child_task_id,
            Some(&delegated_prompt(&subagent_id)),
        )
        .await
        .expect_err("budget escalation denied");
    assert_eq!(error.code, "approval_grant_inheritance_denied");
}

#[tokio::test]
async fn concurrent_child_reservations_do_not_oversubscribe_parent() {
    let (host, context, directory) = parent_fixture().await;
    std::fs::create_dir_all(directory.path().join("src")).expect("scope");
    insert_parent_read_grant(&host, &context, directory.path(), 2, 4, 4096).await;
    let grant = read_grant_request(&directory.path().join("src"));
    let (first_registration, first_id) =
        register_with_grant(&host, &context, "Concurrent reader A", &grant).await;
    let (second_registration, second_id) =
        register_with_grant(&host, &context, "Concurrent reader B", &grant).await;
    let first_task = first_registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("first task")
        .to_owned();
    let second_task = second_registration
        .get("childTaskId")
        .and_then(Value::as_str)
        .expect("second task")
        .to_owned();
    let first_prompt = delegated_prompt(&first_id);
    let second_prompt = delegated_prompt(&second_id);
    let first_context = OperationContext::new(
        "claim-concurrent-a",
        &context.agent_id,
        "agent_user_message",
    );
    let second_context = OperationContext::new(
        "claim-concurrent-b",
        &context.agent_id,
        "agent_user_message",
    );
    let (first, second) = tokio::join!(
        host.claim_subagent_from_message(&first_context, &first_task, Some(&first_prompt)),
        host.claim_subagent_from_message(&second_context, &second_task, Some(&second_prompt))
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let denied = if first.is_err() {
        first.expect_err("first denied")
    } else {
        second.expect_err("second denied")
    };
    assert_eq!(denied.code, "approval_grant_inheritance_denied");
    let used_calls: i64 = sqlx::query_scalar(
        "SELECT used_calls FROM approval_grants WHERE task_id=? AND inherited_from IS NULL",
    )
    .bind(PARENT_TASK_ID)
    .fetch_one(host.repository.pool())
    .await
    .expect("parent usage");
    assert_eq!(used_calls, 2);
}
