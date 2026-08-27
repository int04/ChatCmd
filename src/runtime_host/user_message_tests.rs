use std::{collections::BTreeMap, sync::Arc};

use chatcmd_core::{McpAgentStore as _, NewMcpAgent};
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

async fn test_host() -> (RuntimeHost, String, TempDir) {
    let directory = TempDir::new().expect("temporary directory");
    let (repository, bootstrap) = SqliteRepository::open(&directory.path().join("chatcmd.db"), 2)
        .await
        .expect("open repository");
    let created = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "User message sync test".to_owned(),
            enabled: true,
            project_folder: None,
        })
        .await
        .expect("create agent");
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

fn turn_context(
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
async fn stopped_conversation_cannot_be_resurrected_by_later_mcp_calls() {
    let (host, agent_id, _directory) = test_host().await;
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
    sqlx::query("UPDATE tasks SET status='stopped',stopped_at_ms=1 WHERE id=?")
        .bind(task_id)
        .execute(host.repository.pool())
        .await
        .expect("stop task");

    let error = host
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
        .expect_err("stopped task must remain stopped");
    assert_eq!(error.code, "conversation_stopped");
}
