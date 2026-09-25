use super::*;

async fn fixture_with_limit(
    content: &str,
    limit: i64,
) -> (RuntimeHost, OperationContext, Value, TempDir) {
    let (host, agent, directory) = test_host().await;
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('ui_subagentConcurrency',?,0) \
         ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
    )
    .bind(limit.to_string())
    .execute(host.repository.pool())
    .await
    .unwrap();
    let reply = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "policy-root",
                &agent,
                "agent_user_message",
                "policy-turn",
                "policy-scope",
            ),
            json!({"content":content}),
        )
        .await
        .unwrap();
    let mut context = OperationContext::new("policy-call", agent, "agent_subagent_start");
    context.task_id = Some(reply["taskId"].as_str().unwrap().to_owned());
    context.turn_id = Some("policy-turn".to_owned());
    (host, context, reply, directory)
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

fn assert_enabled_policy(reply: &Value, limit: i64) {
    let policy = &reply["subagentPolicy"];
    assert_eq!(policy["policyVersion"], 2);
    assert_eq!(policy["enabled"], true);
    assert_eq!(policy["maxConcurrent"], limit);
    assert_eq!(policy["delegationAllowed"], true);
    assert_eq!(policy["decisionMode"], "modelJudgment");
    assert_eq!(policy["decisionSource"], "configuredConcurrency");
    assert_eq!(policy["languageIndependent"], true);
    assert_eq!(policy["delegationTextClassifierUsed"], false);
    assert_eq!(policy["authorizationBoundary"]["agent"], "authenticated");
    assert_eq!(policy["authorizationBoundary"]["turn"], "synchronized");
    assert!(policy.get("explicitUserIntent").is_none());
}

#[tokio::test]
async fn delegation_policy_is_identical_for_all_user_text() {
    let messages = [
        "test chia agent, mỗi agent đọc 1 file",
        "请让每个代理读取一个文件",
        "各エージェントが1つのファイルを読んでください",
        "اجعل كل وكيل يقرأ ملفًا واحدًا",
        "Не используйте дочерние агенты",
        "🧪 λ 例 اختبار",
    ];
    for (index, message) in messages.into_iter().enumerate() {
        let (host, context, reply, _directory) = fixture_with_limit(message, 3).await;
        assert_enabled_policy(&reply, 3);
        let run = call(
            &host,
            &context,
            "agent_subagent_start",
            json!({"name":format!("reader-{index}"),"request":"Read one assigned file"}),
        )
        .await
        .unwrap_or_else(|error| {
            panic!("content-independent delegation failed for {message:?}: {error:?}")
        });
        assert!(run["childTaskId"].as_str().is_some());
    }
}

#[tokio::test]
async fn delegation_policy_rejects_an_unsynchronized_turn() {
    let (host, mut context, reply, _directory) = fixture_with_limit("任意文本", 2).await;
    assert_enabled_policy(&reply, 2);
    context.turn_id = Some("not-synchronized".to_owned());
    let error = call(
        &host,
        &context,
        "agent_subagent_start",
        json!({"name":"reader","request":"Read one file"}),
    )
    .await
    .expect_err("an unsynchronized turn must not create a child");
    assert_eq!(error.code, "user_message_sync_required");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn delegation_policy_respects_disabled_runtime_setting() {
    let (host, context, reply, _directory) = fixture_with_limit("分配给多个代理", 0).await;
    let policy = &reply["subagentPolicy"];
    assert_eq!(policy["policyVersion"], 2);
    assert_eq!(policy["enabled"], false);
    assert_eq!(policy["maxConcurrent"], 0);
    assert_eq!(policy["delegationAllowed"], false);
    assert_eq!(policy["decisionMode"], "modelJudgment");
    assert_eq!(policy["decisionSource"], "configuredConcurrency");
    assert_eq!(policy["languageIndependent"], true);
    assert_eq!(policy["delegationTextClassifierUsed"], false);
    let error = call(
        &host,
        &context,
        "agent_subagent_start",
        json!({"name":"reader","request":"Read one file"}),
    )
    .await
    .expect_err("disabled delegation must not create a child");
    assert_eq!(error.code, "subagents_disabled");
}

#[tokio::test]
async fn delegation_contract_validation_remains_independent_of_text_policy() {
    let (host, context, reply, _directory) = fixture_with_limit("ตรวจสอบไฟล์", 1).await;
    assert_enabled_policy(&reply, 1);
    let error = call(
        &host,
        &context,
        "agent_subagent_start",
        json!({
            "name":"reader",
            "request":"Read one file",
            "allowedEffects":[""]
        }),
    )
    .await
    .expect_err("invalid structured constraints must still fail");
    assert_eq!(error.code, "invalid_arguments");
}
