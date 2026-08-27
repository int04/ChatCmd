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
async fn runtime_api_resolves_agent_project_folder() {
    let (host, agent_id, directory) = test_host().await;
    let project = directory.path().join("project-root");
    std::fs::create_dir_all(&project).expect("create project root");
    sqlx::query("UPDATE mcp_agents SET project_folder=? WHERE id=?")
        .bind(project.display().to_string())
        .bind(&agent_id)
        .execute(host.repository.pool())
        .await
        .expect("set project folder");

    let resolved = <RuntimeHost as chatcmd_mcp::RuntimeApi>::project_folder(&host, &agent_id)
        .await
        .expect("resolve project folder");
    assert_eq!(
        resolved.as_deref(),
        Some(project.display().to_string().as_str())
    );
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
async fn chatgpt_bridge_reuses_existing_task_when_mcp_scope_differs_from_url_scope() {
    let (host, agent_id, _directory) = test_host().await;
    let task_id = "task-chatgpt-bridge-existing";
    let request_id = "chatgpt-request-existing";
    let submitted = "Sử dụng plugin @User message sync test để thực hiện yêu cầu sau:\n\nKiểm tra duplicate task";
    let now = now_ms();

    sqlx::query(
        "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web','running',NULL,1,NULL,?,?)",
    )
    .bind(task_id)
    .bind(&agent_id)
    .bind(host.device.id.as_str())
    .bind("openai:url-conversation-id")
    .bind("Kiểm tra duplicate task")
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge task");

    sqlx::query(
        "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,'running',?,?,NULL,NULL,?,?,NULL)",
    )
    .bind(request_id)
    .bind(task_id)
    .bind("chatgpt-turn-existing")
    .bind(&agent_id)
    .bind("Auto")
    .bind("Kiểm tra duplicate task")
    .bind(submitted)
    .bind("conversation-url-id")
    .bind("https://chatgpt.com/c/conversation-url-id")
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge request");

    sqlx::query(
        "INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?)",
    )
    .bind(task_id)
    .bind("conversation-url-id")
    .bind("https://chatgpt.com/c/conversation-url-id")
    .bind("Auto")
    .bind(request_id)
    .bind(now)
    .bind(now)
    .execute(host.repository.pool())
    .await
    .expect("insert bridge conversation");

    let result = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "mcp-user-message",
                &agent_id,
                "agent_user_message",
                "turn-from-openai-session",
                "openai:mcp-session-derived",
            ),
            json!({"content": submitted}),
        )
        .await
        .expect("reuse ChatGPT bridge task");

    assert_eq!(result["taskId"], task_id);
    let row = sqlx::query("SELECT source,conversation_scope_hash FROM tasks WHERE id=?")
        .bind(task_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("read reused bridge task");
    assert_eq!(row.get::<String, _>("source"), "chatgpt_web");
    assert_eq!(
        row.get::<String, _>("conversation_scope_hash"),
        "openai:mcp-session-derived"
    );
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE agent_id=?")
        .bind(&agent_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("count tasks");
    assert_eq!(task_count, 1);
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
async fn delegated_child_keeps_user_message_sync_for_internal_calls() {
    let (host, agent_id, _directory) = test_host().await;
    let parent_scope = "conversation-subagent-internal-sync";
    let parent_turn = "turn-parent-subagent-internal-sync";
    let parent = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "parent-user",
                &agent_id,
                "agent_user_message",
                parent_turn,
                parent_scope,
            ),
            json!({"content":"Create one delegated child"}),
        )
        .await
        .expect("sync parent user message");
    let parent_task = parent["taskId"].as_str().expect("parent task");
    let mut start_context =
        OperationContext::new("subagent-start", &agent_id, "agent_subagent_start");
    start_context.task_id = Some(parent_task.to_owned());
    start_context.turn_id = Some(parent_turn.to_owned());
    let registration = host
        .call_persisted(
            "agent_subagent_start",
            start_context,
            json!({"name":"Reader","request":"Read one file"}),
        )
        .await
        .expect("register child");
    assert_eq!(registration["taskId"], parent_task);
    let child_task = registration["childTaskId"]
        .as_str()
        .expect("child task")
        .to_owned();
    let marker = registration["delegationMarker"].as_str().expect("marker");
    let child_turn = format!(
        "turn-{}",
        registration["subagentId"].as_str().expect("subagent id")
    );

    let mut child_user = OperationContext::new("child-user", &agent_id, "agent_user_message");
    child_user.task_id = Some(child_task.clone());
    child_user.turn_id = Some(child_turn.clone());
    host.call_persisted(
        "agent_user_message",
        child_user,
        json!({"content":format!("Read one file\n\n{marker}")}),
    )
    .await
    .expect("sync child user message");

    let mut roots = OperationContext::new("child-roots", &agent_id, "workspace_roots");
    roots.task_id = Some(child_task);
    roots.turn_id = Some(child_turn);
    host.ensure_call_identity(&mut roots, None)
        .await
        .expect("normalize child internal identity");
    host.ensure_user_message_synced(&roots)
        .await
        .expect("child internal identity must see synchronized user message");
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
