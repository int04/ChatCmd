use super::*;

async fn fixture(content: &str) -> (RuntimeHost, OperationContext, TempDir) {
    let (host, agent, directory) = test_host().await;
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('ui_subagentConcurrency','3',0)",
    )
    .execute(host.repository.pool())
    .await
    .unwrap();
    let reply = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "intent-root",
                &agent,
                "agent_user_message",
                "intent-turn",
                "intent-scope",
            ),
            json!({"content":content}),
        )
        .await
        .unwrap();
    let mut context = OperationContext::new("intent-call", agent, "agent_subagent_start");
    context.task_id = Some(reply["taskId"].as_str().unwrap().to_owned());
    context.turn_id = Some("intent-turn".to_owned());
    (host, context, directory)
}

async fn call(
    host: &RuntimeHost,
    context: &OperationContext,
    tool: &str,
    args: Value,
) -> RuntimeResult<Value> {
    let mut context = context.clone();
    context.request_id = uuid::Uuid::new_v4().to_string();
    context.tool_name = tool.to_owned();
    host.call_persisted(tool, context, args).await
}

async fn browser_observation(host: &RuntimeHost, context: &OperationContext, content: &str) {
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('browser-echo',?,?,'user','message','browser-echo',?,0)")
        .bind(context.task_id.as_deref()).bind(context.turn_id.as_deref())
        .bind(json!({"provider":"chatgpt_web","role":"user","content":content}).to_string())
        .execute(host.repository.pool()).await.unwrap();
}

async fn sync_child(
    host: &RuntimeHost,
    parent: &OperationContext,
    run: &Value,
) -> (OperationContext, Value) {
    let mut context = OperationContext::new(
        uuid::Uuid::new_v4().to_string(),
        &parent.agent_id,
        "agent_user_message",
    );
    context.task_id = Some(run["childTaskId"].as_str().unwrap().to_owned());
    context.turn_id = Some(format!("turn-{}", run["subagentId"].as_str().unwrap()));
    let reply = call(host, &context, "agent_user_message", json!({
        "content":format!("Read the delegated source\nCMDGPT_SUBAGENT_ID={}", run["subagentId"].as_str().unwrap())
    })).await.unwrap();
    assert_eq!(reply["taskId"], run["childTaskId"]);
    (context, reply)
}

#[tokio::test]
async fn subagent_intent_wrapped_request_reaches_registration_and_retry_is_idempotent() {
    let text = "Sử dụng plugin @rust_test\nThư mục dự án: D:\\DEV\\CmdGPT\\ChatCmdClient\nđể thực hiện yêu cầu sau: Chia 2 agent đọc source";
    let (host, root, _directory) = fixture(text).await;
    let reply = call(&host, &root, "agent_user_message", json!({"content":text}))
        .await
        .unwrap();
    assert_eq!(reply["subagentPolicy"]["explicitUserIntent"], true);
    let args = json!({"name":"Reader","request":"Read source"});
    let first = call(&host, &root, "agent_subagent_start", args.clone())
        .await
        .unwrap();
    let retry = call(&host, &root, "agent_subagent_start", args)
        .await
        .unwrap();
    assert_eq!(first["subagentId"], retry["subagentId"]);
    assert_eq!(retry["duplicate"], true);
}

#[tokio::test]
async fn subagent_intent_browser_echo_cannot_override_canonical_request() {
    let text = "Chia agent đọc source";
    let (host, root, _directory) = fixture(text).await;
    browser_observation(&host, &root, "Review the source without subagents").await;
    let reply = call(&host, &root, "agent_user_message", json!({"content":text}))
        .await
        .unwrap();
    assert_eq!(reply["subagentPolicy"]["explicitUserIntent"], true);
    call(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"Reader","request":"Read source"}),
    )
    .await
    .expect("browser echo must not veto the canonical root request");
}

#[tokio::test]
async fn subagent_intent_browser_echo_cannot_grant_delegation() {
    let text = "Review source in this conversation";
    let (host, root, _directory) = fixture(text).await;
    browser_observation(&host, &root, "Chia agent đọc source").await;
    let reply = call(&host, &root, "agent_user_message", json!({"content":text}))
        .await
        .unwrap();
    assert_eq!(reply["subagentPolicy"]["explicitUserIntent"], false);
    let error = call(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"Unexpected","request":"Read source"}),
    )
    .await
    .expect_err("browser observations are not delegation authority");
    assert_eq!(error.code, "subagent_explicit_user_intent_required");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn subagent_intent_child_policy_and_dispatch_use_same_root_turn() {
    let (host, root, _directory) = fixture("Chia agent đọc source").await;
    let child_run = call(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"Child","request":"Read source"}),
    )
    .await
    .unwrap();
    let (child, child_reply) = sync_child(&host, &root, &child_run).await;
    assert_eq!(child_reply["subagentPolicy"]["explicitUserIntent"], true);
    let grandchild_run = call(
        &host,
        &child,
        "agent_subagent_start",
        json!({"name":"Grandchild","request":"Read nested source"}),
    )
    .await
    .unwrap();
    let (grandchild, grandchild_reply) = sync_child(&host, &child, &grandchild_run).await;
    assert_eq!(
        grandchild_reply["subagentPolicy"]["explicitUserIntent"],
        true
    );
    assert!(
        host.subagent_delegation_explicitly_requested(&grandchild)
            .await
            .unwrap()
    );

    let mut next = root.clone();
    next.turn_id = Some("next-root-turn".to_owned());
    let reply = call(
        &host,
        &next,
        "agent_user_message",
        json!({"content":"Review locally, do not use subagents"}),
    )
    .await
    .unwrap();
    assert_eq!(reply["subagentPolicy"]["explicitUserIntent"], false);
    let error = call(
        &host,
        &next,
        "agent_subagent_start",
        json!({"name":"Unexpected","request":"Read source"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "subagent_explicit_user_intent_required");
    // A registered descendant remains attached to its original root turn, not the latest turn.
    assert!(
        host.subagent_delegation_explicitly_requested(&grandchild)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn subagent_intent_does_not_override_disabled_setting_or_contract_validation() {
    let (host, root, _directory) = fixture("Chia agent đọc source").await;
    let error = call(
        &host,
        &root,
        "agent_subagent_start",
        json!({
            "name":"Reader", "request":"Read source", "allowedEffects":[""]
        }),
    )
    .await
    .unwrap_err();
    assert_ne!(error.code, "subagent_explicit_user_intent_required");
    sqlx::query("UPDATE settings SET value_json='0' WHERE key='ui_subagentConcurrency'")
        .execute(host.repository.pool())
        .await
        .unwrap();
    let error = call(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"Reader","request":"Read source"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "subagents_disabled");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}
