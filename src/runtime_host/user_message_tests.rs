use std::{collections::BTreeMap, sync::Arc};

use chatcmd_core::{McpAgentStore as _, NewMcpAgent, ToolCatalogStore as _};
use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, NullEventSink, PolicyContext, PolicyDecision,
    PolicyEngine, RuntimeConfig,
};
use chatcmd_storage::SqliteRepository;
use serde_json::json;
use sqlx::Row as _;
use tempfile::TempDir;
use tokio::sync::broadcast;

use super::*;

struct AllowApproval;

impl ApprovalDecision for AllowApproval {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

pub(crate) async fn test_host() -> (RuntimeHost, String, TempDir) {
    let directory = TempDir::new().expect("temporary directory");
    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .expect("open repository");
    sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES('ui_approveNewConversations','false',0) ON CONFLICT(key) DO UPDATE SET value_json='false',updated_at_ms=0")
        .execute(repository.pool())
        .await
        .expect("disable conversation approval for runtime tests");
    let created = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "User message sync test".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent");
    let fs_read_text_tool_id = repository
        .list_tools()
        .await
        .expect("list tools")
        .into_iter()
        .find(|tool| tool.key == "fs_read_text")
        .expect("fs_read_text tool")
        .id;
    repository
        .set_agent_allowed_tools(&created.agent.id, &[fs_read_text_tool_id])
        .await
        .expect("allow fs_read_text");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical test root");
    let policy = PolicyEngine::new(
        Some(ExecutionPolicy {
            default: PolicyDecision::Allow,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }),
        Arc::new(AllowApproval),
    );
    let workspace = WorkspaceService::new(std::slice::from_ref(&root), policy.clone())
        .expect("workspace service");
    let config = RuntimeConfig {
        roots: vec![root.clone()],
        repository_root: Some(root.clone()),
        ..RuntimeConfig::default()
    };
    let shell = ShellRuntime::new(config, policy.clone(), Arc::new(NullEventSink));
    let git = GitService::new(workspace.clone(), 10_000);
    let process = ProcessService::new(policy);
    let skills = SkillService::new(None, Some(&root), 10_000);
    let (events, _) = broadcast::channel(16);
    (
        RuntimeHost::new(
            repository,
            bootstrap.device,
            0,
            shell,
            workspace,
            git,
            process,
            skills,
            events,
        ),
        created.agent.id.as_str().to_owned(),
        directory,
    )
}

pub(super) fn turn_context(
    request_id: &str,
    agent_id: &str,
    tool: &str,
    turn_id: &str,
    conversation_scope: &str,
) -> OperationContext {
    let mut context = OperationContext::new(request_id, agent_id, tool);
    context.turn_id = Some(turn_id.to_owned());
    context.conversation_scope_id = Some(conversation_scope.to_owned());
    context
}

#[tokio::test]
async fn user_message_is_required_first_and_is_idempotent_per_turn() {
    let (host, agent_id, _directory) = test_host().await;
    let scope = "conversation-user-message-sync";
    let turn = "turn-user-message-sync";

    let error = host
        .call_persisted(
            "agent_progress",
            turn_context(
                "progress-before-user",
                &agent_id,
                "agent_progress",
                turn,
                scope,
            ),
            json!({"message":"should be rejected"}),
        )
        .await
        .expect_err("progress before user message must be rejected");
    assert_eq!(error.code, "user_message_sync_required");

    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "user-message-first",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Nguyên văn tin nhắn người dùng"}),
        )
        .await
        .expect("sync user message");
    assert_eq!(accepted["userMessageSynced"], true);
    assert_eq!(accepted["duplicate"], false);
    assert_eq!(accepted["isFirstMessage"], true);
    assert_eq!(accepted["toolRecovery"]["catalogIsStable"], true);
    assert_eq!(accepted["toolRecovery"]["hostMayLazyLoadSchemas"], true);
    assert_eq!(
        accepted["toolRecovery"]["missingSchemaDoesNotMeanMissingTool"],
        true
    );
    assert_eq!(
        accepted["toolRecovery"]["mustDiscoverBeforeUnavailableReply"],
        true
    );
    assert_eq!(accepted["toolRecovery"]["mustContinueInSameTurn"], true);
    assert!(
        accepted["toolRecovery"]["chatGptDiscoveryHint"]
            .as_str()
            .is_some_and(|value| value.contains("api_tool.list_resources"))
    );
    assert_eq!(accepted["toolRecovery"]["recommendedQueries"][0], "fs_");
    let task_id = accepted["taskId"].as_str().expect("task ID").to_owned();
    let turn_id = accepted["turnId"].as_str().expect("turn ID").to_owned();

    host.call_persisted(
        "agent_progress",
        turn_context(
            "progress-after-user",
            &agent_id,
            "agent_progress",
            turn,
            scope,
        ),
        json!({"message":"now accepted"}),
    )
    .await
    .expect("progress after user message");

    let duplicate = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "user-message-retry",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Nguyên văn tin nhắn người dùng"}),
        )
        .await
        .expect("idempotent retry");
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["isFirstMessage"], true);
    assert_eq!(
        duplicate["toolRecovery"]["mustDiscoverBeforeUnavailableReply"],
        true
    );

    let conflict = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "user-message-conflict",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Nội dung khác"}),
        )
        .await
        .expect_err("same turn cannot be rebound to different user text");
    assert_eq!(conflict.code, "turn_user_message_conflict");

    let row = sqlx::query(
        "SELECT COUNT(*) AS count, MIN(payload_json) AS payload_json FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='user' AND kind='message'",
    )
    .bind(&task_id)
    .bind(&turn_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("read stored user message");
    assert_eq!(row.get::<i64, _>("count"), 1);
    let payload: serde_json::Value = serde_json::from_str(&row.get::<String, _>("payload_json"))
        .expect("stored user message payload");
    assert_eq!(payload["role"], "user");
    assert_eq!(payload["content"], "Nguyên văn tin nhắn người dùng");
}

#[tokio::test]
async fn first_message_seeds_task_id_and_only_first_final_can_name_chat() {
    let (host, agent_id, _directory) = test_host().await;
    let scope = "conversation-first-message-identity";
    let first_turn = "turn-first";
    let first_text = "Khắc phục lỗi git diff stat trong dự án";

    let first = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "first-user",
                &agent_id,
                "agent_user_message",
                first_turn,
                scope,
            ),
            json!({"content":first_text}),
        )
        .await
        .expect("first user message");
    assert_eq!(first["isFirstMessage"], true);
    assert_eq!(first["suggestedTitleRequired"], true);
    assert_eq!(first["provisionalTitle"], first_text);
    let task_id = first["taskId"].as_str().expect("task id").to_owned();
    assert!(task_id.starts_with("task-chat-"));

    let provisional_title = sqlx::query_scalar::<_, String>("SELECT title FROM tasks WHERE id=?")
        .bind(&task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("provisional title");
    assert_eq!(provisional_title, first_text);

    let completed = host
        .call_persisted(
            "agent_turn_complete",
            turn_context(
                "first-complete",
                &agent_id,
                "agent_turn_complete",
                first_turn,
                scope,
            ),
            json!({"content":"Đã xử lý xong.", "suggestedTitle":"Sửa lỗi Git diff stat"}),
        )
        .await
        .expect("first completion");
    assert_eq!(completed["titleUpdated"], true);

    let second_turn = "turn-second";
    let second = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "second-user",
                &agent_id,
                "agent_user_message",
                second_turn,
                scope,
            ),
            json!({"content":"Commit thay đổi"}),
        )
        .await
        .expect("second user message");
    assert_eq!(second["taskId"], task_id);
    assert_eq!(second["isFirstMessage"], false);
    assert_eq!(second["suggestedTitleRequired"], false);

    let second_completed = host
        .call_persisted(
            "agent_turn_complete",
            turn_context(
                "second-complete",
                &agent_id,
                "agent_turn_complete",
                second_turn,
                scope,
            ),
            json!({"content":"Đã commit.", "suggestedTitle":"Tên này không được áp dụng"}),
        )
        .await
        .expect("second completion");
    assert_eq!(second_completed["titleUpdated"], false);

    let final_title = sqlx::query_scalar::<_, String>("SELECT title FROM tasks WHERE id=?")
        .bind(&task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("final title");
    assert_eq!(final_title, "Sửa lỗi Git diff stat");
}

#[tokio::test]
async fn repeated_subagent_registration_is_idempotent_with_new_request_id() {
    let (host, agent_id, _directory) = test_host().await;
    let scope = "conversation-subagent-idempotency";
    let turn = "turn-subagent-idempotency";
    let parent = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "parent-user-idempotency",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Create delegated reviewer"}),
        )
        .await
        .expect("sync parent");
    let parent_task = parent["taskId"].as_str().expect("parent task");

    let start = |request_id: &str| {
        let mut context = OperationContext::new(request_id, &agent_id, "agent_subagent_start");
        context.task_id = Some(parent_task.to_owned());
        context.turn_id = Some(turn.to_owned());
        context
    };
    let first = host
        .call_persisted(
            "agent_subagent_start",
            start("start-first"),
            json!({"name":"MCP Lib Reviewer","request":"Read exactly lib.rs"}),
        )
        .await
        .expect("first registration");
    let retry = host
        .call_persisted(
            "agent_subagent_start",
            start("start-retry-with-new-request-id"),
            json!({"name":"MCP Lib Reviewer","request":"Read exactly lib.rs"}),
        )
        .await
        .expect("idempotent retry");

    assert_eq!(first["taskId"], parent_task);
    assert_eq!(retry["taskId"], parent_task);
    assert_eq!(first["childTaskId"], retry["childTaskId"]);
    assert_eq!(first["subagentId"], retry["subagentId"]);
    assert_eq!(first["duplicate"], false);
    assert_eq!(retry["duplicate"], true);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM subagent_runs WHERE parent_task_id=? AND parent_turn_id=? AND name=? AND request=?",
    )
    .bind(parent_task)
    .bind(turn)
    .bind("MCP Lib Reviewer")
    .bind("Read exactly lib.rs")
    .fetch_one(host.repository.pool())
    .await
    .expect("count subagents");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn stopped_conversation_reopens_with_a_new_logical_session() {
    let (host, agent_id, _directory) = test_host().await;
    sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES('ui_approveNewConversations','false',0) ON CONFLICT(key) DO UPDATE SET value_json='false',updated_at_ms=0")
        .execute(host.repository.pool())
        .await
        .expect("disable conversation approval for test");
    let scope = "conversation-stop-guard";
    let first = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "stop-user",
                &agent_id,
                "agent_user_message",
                "turn-stop-1",
                scope,
            ),
            json!({"content":"Start conversation"}),
        )
        .await
        .expect("initial user message");
    let task_id = first["taskId"].as_str().expect("task id");
    let first_session_id = first["sessionId"].as_str().expect("session id").to_owned();
    sqlx::query(
        "UPDATE tasks SET status='stopped',active_session_id=NULL,stopped_at_ms=1 WHERE id=?",
    )
    .bind(task_id)
    .execute(host.repository.pool())
    .await
    .expect("stop task");

    let stopped_agent_call = host
        .call_persisted(
            "agent_progress",
            turn_context(
                "stop-agent-progress",
                &agent_id,
                "agent_progress",
                "turn-stop-agent",
                scope,
            ),
            json!({"message":"Agent is still working"}),
        )
        .await
        .expect_err("agent calls must not reopen a stopped conversation");
    assert_eq!(stopped_agent_call.code, "conversation_stopped");

    let reopened = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "stop-user-next",
                &agent_id,
                "agent_user_message",
                "turn-stop-2",
                scope,
            ),
            json!({"content":"Try to continue"}),
        )
        .await
        .expect("a new user message should reopen the stopped task");
    assert_eq!(reopened["taskId"], task_id);
    assert_ne!(reopened["sessionId"], first_session_id);

    let task = sqlx::query(
        "SELECT status,generation,stopped_at_ms,active_session_id FROM tasks WHERE id=?",
    )
    .bind(task_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("read reopened task");
    assert_eq!(
        task.try_get::<String, _>("status").expect("status"),
        "running"
    );
    assert_eq!(task.try_get::<i64, _>("generation").expect("generation"), 2);
    assert!(
        task.try_get::<Option<i64>, _>("stopped_at_ms")
            .expect("stopped_at_ms")
            .is_none()
    );
    assert_eq!(
        task.try_get::<Option<String>, _>("active_session_id")
            .expect("active_session_id")
            .as_deref(),
        reopened["sessionId"].as_str()
    );

    let continued = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "stop-user-third",
                &agent_id,
                "agent_user_message",
                "turn-stop-3",
                scope,
            ),
            json!({"content":"Run a command"}),
        )
        .await
        .expect("later turns after reopen should reuse generation session without conflict");
    assert_eq!(continued["taskId"], task_id);
    assert_eq!(continued["sessionId"], reopened["sessionId"]);
}
