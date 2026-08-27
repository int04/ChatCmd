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

fn turn_context(request_id: &str, agent_id: &str, tool: &str) -> OperationContext {
    let mut context = OperationContext::new(request_id, agent_id, tool);
    context.turn_id = Some("turn-user-message-sync".to_owned());
    context.conversation_scope_id = Some("conversation-user-message-sync".to_owned());
    context
}

#[tokio::test]
async fn user_message_is_required_first_and_is_idempotent_per_turn() {
    let (host, agent_id, _directory) = test_host().await;

    let error = host
        .call_persisted(
            "agent_progress",
            turn_context("progress-before-user", &agent_id, "agent_progress"),
            json!({"message":"should be rejected"}),
        )
        .await
        .expect_err("progress before user message must be rejected");
    assert_eq!(error.code, "user_message_sync_required");

    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context("user-message-first", &agent_id, "agent_user_message"),
            json!({"content":"Nguyên văn tin nhắn người dùng"}),
        )
        .await
        .expect("sync user message");
    assert_eq!(accepted["userMessageSynced"], true);
    assert_eq!(accepted["duplicate"], false);
    let task_id = accepted["taskId"].as_str().expect("task ID").to_owned();
    let turn_id = accepted["turnId"].as_str().expect("turn ID").to_owned();

    host.call_persisted(
        "agent_progress",
        turn_context("progress-after-user", &agent_id, "agent_progress"),
        json!({"message":"now accepted"}),
    )
    .await
    .expect("progress after user message");

    let duplicate = host
        .call_persisted(
            "agent_user_message",
            turn_context("user-message-retry", &agent_id, "agent_user_message"),
            json!({"content":"Nguyên văn tin nhắn người dùng"}),
        )
        .await
        .expect("idempotent retry");
    assert_eq!(duplicate["duplicate"], true);

    let conflict = host
        .call_persisted(
            "agent_user_message",
            turn_context("user-message-conflict", &agent_id, "agent_user_message"),
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
