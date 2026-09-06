use super::*;

fn call<'a>(
    host: &'a RuntimeHost,
    context: &'a OperationContext,
    tool: &'a str,
    args: Value,
) -> chatcmd_runtime::BoxFuture<'a, chatcmd_runtime::RuntimeResult<Value>> {
    let mut context = context.clone();
    context.request_id = format!("grant-test-{tool}-{}", uuid::Uuid::new_v4());
    context.tool_name = tool.to_owned();
    Box::pin(host.call_persisted(tool, context, args))
}

#[tokio::test]
async fn valid_grants_reach_grandchild_with_two_slots_and_preserve_budgets_and_reports() {
    let (host, parent, directory) = parent_fixture().await;
    sqlx::query("UPDATE settings SET value_json='2' WHERE key='ui_subagentConcurrency'")
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
    let root_grant =
        insert_parent_read_grant(&host, &parent, directory.path(), 32, 100, 16_777_216).await;
    let path = directory.path().join("nested.txt");
    std::fs::write(&path, "nested read").unwrap();
    let mut grant = read_grant_request(directory.path());
    grant.allowed_tools = vec!["fs_read_text".into()];
    grant.max_calls = 8;
    grant.max_files_scanned = 16;
    grant.max_bytes_read = 4_194_304;
    let child_run = host
        .register_subagent(&parent, "Child", "Read and delegate", Some(&grant))
        .await
        .unwrap();
    let (child, first) = sync_child(&host, &parent, &child_run).await;
    assert_eq!(first["subagentApproval"]["status"], "inherited");
    assert!(
        first["subagentPolicy"]["approvalGrant"]["allowedTools"]
            .as_array()
            .unwrap()
            .contains(&json!("fs_read_text"))
    );
    assert!(
        !first["subagentPolicy"]["approvalGrant"]["allowedTools"]
            .as_array()
            .unwrap()
            .contains(&json!("git_status"))
    );
    let read = call(
        &host,
        &child,
        "fs_read_text",
        json!({"path":path,"maxCharacters":32}),
    )
    .await
    .unwrap();
    assert_eq!(read["content"], "nested read");
    grant.max_calls = 2;
    grant.max_files_scanned = 4;
    grant.max_bytes_read = 1_048_576; // Legacy read adapter reserves up to 1 MiB per call.
    let grandchild_run = call(
        &host,
        &child,
        "agent_subagent_start",
        json!({"name":"Grandchild","request":"Read nested file","approvalGrant":grant}),
    )
    .await
    .unwrap();
    let (grandchild, nested) = sync_child(&host, &child, &grandchild_run).await;
    assert_eq!(nested["userMessageSynced"], true);
    assert_eq!(nested["subagentApproval"]["status"], "inherited");
    let read = call(
        &host,
        &grandchild,
        "fs_read_text",
        json!({"path":path,"maxCharacters":32}),
    )
    .await
    .unwrap();
    assert_eq!(read["content"], "nested read");
    let child_grant = first["subagentApproval"]["grantId"].as_str().unwrap();
    let grandchild_grant = nested["subagentApproval"]["grantId"].as_str().unwrap();
    let row = sqlx::query("SELECT g.inherited_from,g.expires_at_ms,p.expires_at_ms AS parent_expiry FROM approval_grants g JOIN approval_grants p ON p.id=g.inherited_from WHERE g.id=?")
        .bind(grandchild_grant).fetch_one(host.repository.pool()).await.unwrap();
    assert_eq!(row.get::<String, _>("inherited_from"), child_grant);
    assert!(row.get::<i64, _>("expires_at_ms") <= row.get::<i64, _>("parent_expiry"));
    let used: i64 = sqlx::query_scalar("SELECT used_calls FROM approval_grants WHERE id=?")
        .bind(&root_grant)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(
        used, 8,
        "grandchild reserves from immediate parent, not root twice"
    );
    let used: i64 = sqlx::query_scalar("SELECT used_calls FROM approval_grants WHERE id=?")
        .bind(child_grant)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(used, 3, "one read plus two calls reserved for grandchild");
    let (_, repeated) = sync_child(&host, &child, &grandchild_run).await;
    assert_eq!(repeated["subagentApproval"], nested["subagentApproval"]);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_grants WHERE task_id=?")
        .bind(grandchild.task_id.as_deref())
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let wait = host.wait_for_subagents(&parent, 250).await.unwrap();
    assert_eq!(wait["activeCount"], 2);
    assert!(
        wait["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["approvalGrant"]["status"] == "inherited")
    );
    for (context, content) in [
        (&grandchild, "Grandchild read nested.txt"),
        (&child, "Child reviewed grandchild report"),
    ] {
        call(
            &host,
            context,
            "agent_turn_complete",
            json!({"content":content,"workOutcome":"completed"}),
        )
        .await
        .unwrap();
    }
    let wait = host.wait_for_subagents(&parent, 250).await.unwrap();
    assert_eq!(wait["allFinished"], true);
    assert!(
        wait["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["report"]["content"] == "Grandchild read nested.txt")
    );
}

#[tokio::test]
async fn stale_or_unavailable_parent_grants_cannot_authorize_grandchildren() {
    for fault in [
        "revoked",
        "expired",
        "attempt",
        "turn",
        "catalog",
        "exhausted",
    ] {
        let (host, parent, directory) = parent_fixture().await;
        insert_parent_read_grant(&host, &parent, directory.path(), 32, 100, 16_777_216).await;
        let mut grant = read_grant_request(directory.path());
        grant.allowed_tools = vec!["fs_read_text".into()];
        grant.max_calls = 8;
        grant.max_files_scanned = 16;
        grant.max_bytes_read = 4_194_304;
        let child_run = host
            .register_subagent(&parent, "Child", "Delegate", Some(&grant))
            .await
            .unwrap();
        let (child, first) = sync_child(&host, &parent, &child_run).await;
        let child_grant = first["subagentApproval"]["grantId"].as_str().unwrap();
        let assignment = match fault {
            "revoked" => "state='revoked'",
            "expired" => "expires_at_ms=0",
            "attempt" => "child_attempt=999",
            "turn" => "turn_id='other-turn'",
            "catalog" => "catalog_hash='stale'",
            _ => "used_calls=max_calls",
        };
        sqlx::query(&format!(
            "UPDATE approval_grants SET {assignment} WHERE id=?"
        ))
        .bind(child_grant)
        .execute(host.repository.pool())
        .await
        .unwrap();
        grant.max_calls = 1;
        grant.max_files_scanned = 1;
        grant.max_bytes_read = 128;
        let run = host
            .register_subagent(&child, "Grandchild", "Read", Some(&grant))
            .await
            .unwrap();
        let (nested, reply) = sync_child(&host, &child, &run).await;
        assert_eq!(
            reply["subagentApproval"]["status"], "notInherited",
            "{fault}"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_grants WHERE task_id=?")
            .bind(nested.task_id.as_deref())
            .fetch_one(host.repository.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{fault} parent cannot mint a grant");
        sqlx::query(
            "INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,'deny',0)",
        )
        .bind(PARENT_TASK_ID)
        .execute(host.repository.pool())
        .await
        .unwrap();
        for tool in ["fs_read_text", "fs_write_text", "git_status", "command_run"] {
            let error = host
                .authorize_execution(&nested, tool, &json!({"path":directory.path()}))
                .await
                .unwrap_err();
            assert_eq!(
                error.code, "policy_denied",
                "root deny remains authoritative for {tool}"
            );
        }
    }
}

#[tokio::test]
async fn storage_error_rolls_back_claim_and_reservation_instead_of_hiding_failure() {
    let (host, parent, directory) = parent_fixture().await;
    let parent_grant =
        insert_parent_read_grant(&host, &parent, directory.path(), 8, 32, 65_536).await;
    let grant = read_grant_request(directory.path());
    let (run, id) = register_with_grant(&host, &parent, "Reader", &grant).await;
    sqlx::query("CREATE TRIGGER deny_grant_diagnostic BEFORE INSERT ON timeline_events WHEN json_extract(NEW.payload_json,'$.status')='subagent_approval' BEGIN SELECT RAISE(ABORT,'injected failure'); END")
        .execute(host.repository.pool()).await.unwrap();
    let mut context =
        OperationContext::new("storage-claim", &parent.agent_id, "agent_user_message");
    context.turn_id = Some(format!("turn-{id}"));
    let error = host
        .claim_subagent_from_message(
            &context,
            run["childTaskId"].as_str().unwrap(),
            Some(&delegated_prompt(&id)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "storage_error");
    let row = sqlx::query("SELECT used_calls FROM approval_grants WHERE id=?")
        .bind(parent_grant)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("used_calls"), 0);
    let state: String = sqlx::query_scalar("SELECT status FROM subagent_runs WHERE id=?")
        .bind(id)
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(state, "pending");
}
