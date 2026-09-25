//! Routed ChatUI turns use structured delegation policy, never text classification.
use super::routing_tests::{REQUEST, SCOPE, TASK, TURN, seed};
use crate::runtime_host::{
    RuntimeHost,
    user_message_tests::{test_host, turn_context},
};
use chatcmd_runtime::OperationContext;
use serde_json::{Value, json};
use tempfile::TempDir;

async fn fixture(original: &str) -> (RuntimeHost, String, TempDir) {
    let (host, agent, directory) = test_host().await;
    let submitted = crate::chatgpt_routing::with_route(original, REQUEST, TURN);
    seed(&host, &agent, &submitted).await;
    sqlx::query("UPDATE chatgpt_bridge_requests SET user_content=? WHERE id=?")
        .bind(original)
        .bind(REQUEST)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET project_folder=? WHERE id=?")
        .bind(directory.path().to_str().unwrap())
        .bind(TASK)
        .execute(host.repository.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings(key,value_json,updated_at_ms) VALUES('ui_subagentConcurrency','2',0)",
    )
    .execute(host.repository.pool())
    .await
    .unwrap();
    // Browser observations precede MCP echoes, as in the pre-3bfb9fc9 flow.
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('browser-original',?,?,'user','message','browser-original',?,0)")
        .bind(TASK).bind(TURN).bind(json!({"provider":"chatgpt_web","content":original}).to_string())
        .execute(host.repository.pool()).await.unwrap();
    (host, agent, directory)
}

async fn invoke(
    host: &RuntimeHost,
    context: &OperationContext,
    tool: &str,
    args: Value,
) -> chatcmd_runtime::RuntimeResult<Value> {
    let mut context = context.clone();
    context.request_id = uuid::Uuid::new_v4().to_string();
    context.tool_name = tool.into();
    host.call_persisted(tool, context, args).await
}

async fn delegation_allowed_for_echo(original: &str, echo: &str) -> bool {
    let (host, agent, _dir) = fixture(original).await;
    let reply = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "echo-initialization",
                &agent,
                "agent_user_message",
                TURN,
                SCOPE,
            ),
            json!({"content":echo}),
        )
        .await
        .unwrap();
    assert_eq!(reply["userMessageSynced"], true);
    assert_eq!(reply["taskId"], TASK);
    assert_eq!(
        reply["subagentPolicy"]["delegationTextClassifierUsed"],
        false
    );
    reply["subagentPolicy"]["delegationAllowed"]
        .as_bool()
        .unwrap()
}

#[tokio::test]
async fn subagent_routed_policy_survives_shortened_mcp_echo() {
    assert!(
        delegation_allowed_for_echo(
            "Chia ra agent đọc hai file, không sửa file",
            "Đọc hai file, không sửa file"
        )
        .await
    );
}

#[tokio::test]
async fn subagent_routed_policy_does_not_classify_original_or_echo_text() {
    assert!(
        delegation_allowed_for_echo(
            "Không chia agent, chỉ đọc một file trong cuộc trò chuyện này",
            "Chia ra agent đọc file"
        )
        .await
    );
}

#[tokio::test]
async fn subagent_routed_two_readers_sync_progress_read_finish_and_parent_wait() {
    let (host, agent, directory) =
        fixture("Chia ra 2 agent để thực hiện yêu cầu sau: đọc hai file, không sửa file").await;
    let mut root = turn_context("root-start", &agent, "agent_user_message", TURN, SCOPE);
    let synced = invoke(
        &host,
        &root,
        "agent_user_message",
        json!({"content":"Đọc hai file"}),
    )
    .await
    .unwrap();
    assert_eq!(synced["subagentPolicy"]["delegationAllowed"], true);
    root.task_id = Some(TASK.into());
    invoke(
        &host,
        &root,
        "agent_progress",
        json!({"message":"Starting two readers"}),
    )
    .await
    .unwrap();
    let scope = std::fs::canonicalize(directory.path()).unwrap();
    let normalized = scope.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let normalized = normalized.to_ascii_lowercase();
    let metadata = std::fs::metadata(&scope).unwrap();
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt as _;
        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!(
        "created:{}:dir:{}",
        metadata
            .created()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        metadata.is_dir()
    );
    // A test-only approved parent grant, reserved into exactly one file per child.
    let now = super::now_ms();
    sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,turn_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read,expires_at_ms,catalog_hash,state,created_at_ms,updated_at_ms) VALUES('two-reader-grant',?,?,?, ?,?, ?,2,2,2097152,?,?,'active',?,?)")
        .bind(&agent).bind(TASK).bind(TURN).bind(json!(["fs_read_text"]).to_string())
        .bind(json!([{"path":normalized,"kind":"subtree","identity":identity}]).to_string())
        .bind(json!({"includeIgnored":false,"includeHidden":false}).to_string())
        .bind(now+600_000).bind(chatcmd_mcp::catalog_hash()).bind(now).bind(now)
        .execute(host.repository.pool()).await.unwrap();
    let mut runs = Vec::new();
    for name in ["analysis_options.yaml", "pubspec.yaml"] {
        let file = directory.path().join(name);
        tokio::fs::write(&file, format!("name: {name}\n"))
            .await
            .unwrap();
        let run = invoke(
            &host,
            &root,
            "agent_subagent_start",
            json!({
                "name":name,"request":format!("Read only {}", file.display()),
                "allowedFiles":[file],"allowedEffects":["read"],
                "approvalGrant":{"allowedTools":["fs_read_text"],"pathScopes":[file],
                    "maxCalls":1,"maxFilesScanned":1,"maxBytesRead":1048576}
            }),
        )
        .await
        .unwrap();
        runs.push((run, file));
    }
    async fn reader(host: &RuntimeHost, agent: &str, run: &Value, file: &std::path::Path) {
        let id = run["subagentId"].as_str().unwrap();
        let task = run["childTaskId"].as_str().unwrap();
        let mut context = turn_context(
            "child-sync",
            agent,
            "agent_user_message",
            &format!("turn-{id}"),
            &format!("openai:child-{id}"),
        );
        context.task_id = Some(task.into());
        let prompt = crate::runtime_host::subagent_fallback::browser_subagent_prompt(
            Some("reader"),
            &format!("Read only {}", file.display()),
            id,
            task,
        );
        let sync = invoke(
            host,
            &context,
            "agent_user_message",
            json!({"content":prompt}),
        )
        .await
        .unwrap();
        assert_eq!(sync["userMessageSynced"], true);
        assert_eq!(sync["taskId"], task);
        assert_eq!(sync["subagentApproval"]["status"], "inherited", "{sync}");
        invoke(
            host,
            &context,
            "agent_progress",
            json!({"message":"Reading assigned file"}),
        )
        .await
        .unwrap();
        let read = invoke(
            host,
            &context,
            "fs_read_text",
            json!({"path":file,"maxCharacters":2000}),
        )
        .await
        .unwrap();
        let expected = format!("name: {}\n", file.file_name().unwrap().to_str().unwrap());
        assert_eq!(read["content"].as_str(), Some(expected.as_str()));
        let final_result = invoke(host, &context, "agent_turn_complete", json!({
            "content":format!("Read {} without modifying it",file.file_name().unwrap().to_str().unwrap()),
            "workOutcome":"completed","verificationIntent":"notApplicable","verificationReason":"Read-only fixture"
        })).await.unwrap();
        assert_eq!(final_result["accepted"], true);
        assert_eq!(tokio::fs::read_to_string(file).await.unwrap(), expected);
    }
    tokio::join!(
        reader(&host, &agent, &runs[0].0, &runs[0].1),
        reader(&host, &agent, &runs[1].0, &runs[1].1)
    );
    let report = invoke(&host, &root, "agent_subagent_wait", json!({"timeoutMs":10}))
        .await
        .unwrap();
    assert_eq!(report["allFinished"], true);
    let children = report["subagents"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    for child in children {
        assert_eq!(child["report"]["source"], "mcpFinal");
        assert_eq!(child["report"]["mcpFinalizerReceived"], true);
    }
    assert_eq!(
        invoke(
            &host,
            &root,
            "agent_turn_complete",
            json!({"content":"Both readers completed","workOutcome":"completed"})
        )
        .await
        .unwrap()["accepted"],
        true
    );
}

#[tokio::test]
async fn subagent_registration_is_not_gated_by_vietnamese_text() {
    let (host, agent, _dir) = fixture("Đọc file trong cuộc trò chuyện này, không chia agent").await;
    let mut root = turn_context(
        "root-no-delegation",
        &agent,
        "agent_user_message",
        TURN,
        SCOPE,
    );
    invoke(
        &host,
        &root,
        "agent_user_message",
        json!({"content":"Đọc một file"}),
    )
    .await
    .unwrap();
    root.task_id = Some(TASK.into());
    let run = invoke(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"model-selected-reader","request":"Read file"}),
    )
    .await
    .expect("enabled synchronized turn may delegate without a text classifier");
    assert!(run["childTaskId"].as_str().is_some());
}

#[tokio::test]
async fn subagent_routed_policy_is_bound_to_the_synchronized_child_turn() {
    let (host, agent, _dir) = fixture("Chia ra agent đọc file").await;
    let mut root = turn_context(
        "root-original-turn",
        &agent,
        "agent_user_message",
        TURN,
        SCOPE,
    );
    invoke(
        &host,
        &root,
        "agent_user_message",
        json!({"content":"Đọc file"}),
    )
    .await
    .unwrap();
    root.task_id = Some(TASK.into());
    let run = invoke(
        &host,
        &root,
        "agent_subagent_start",
        json!({"name":"reader","request":"Read file"}),
    )
    .await
    .unwrap();
    let later =
        crate::chatgpt_routing::with_route("Không chia agent", "later-request", "later-turn");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,created_at_ms,updated_at_ms) VALUES('later-request',?,'later-turn',?,'Auto','Không chia agent',?,'running',?,?)")
        .bind(TASK).bind(&agent).bind(later).bind(super::now_ms()+1).bind(super::now_ms()+1)
        .execute(host.repository.pool()).await.unwrap();
    let mut child = turn_context(
        "child-old-root",
        &agent,
        "agent_user_message",
        "child-turn",
        "openai:child-old-root",
    );
    child.task_id = run["childTaskId"].as_str().map(str::to_owned);
    let prompt = format!(
        "Read file\nCMDGPT_SUBAGENT_ID={}",
        run["subagentId"].as_str().unwrap()
    );
    let synced = invoke(
        &host,
        &child,
        "agent_user_message",
        json!({"content":prompt}),
    )
    .await
    .unwrap();
    assert_eq!(synced["subagentPolicy"]["delegationAllowed"], true);
}
