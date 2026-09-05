use std::{collections::BTreeMap, sync::Arc};

use chatcmd_core::{McpAgentStore as _, NewMcpAgent, ToolCatalogStore as _};
use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, CommandExecutionService, ExecutionPolicy, NullEventSink,
    PolicyContext, PolicyDecision, PolicyEngine, RuntimeConfig,
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
    let command = CommandExecutionService::new(
        workspace.clone(),
        Arc::new(policy.clone()),
        root.join(".test-command-artifacts"),
        2,
    );
    let process = ProcessService::new(policy);
    let skills = SkillService::new(None, Some(&root), 10_000);
    let (events, _) = broadcast::channel(16);
    (
        RuntimeHost::new(
            repository,
            bootstrap.device,
            shell,
            workspace,
            chatcmd_runtime::BlobStore::new(root.join(".test-blobs")).expect("blob store"),
            git,
            command,
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
async fn large_tool_content_is_absent_from_sqlite_and_realtime() {
    let (host, agent_id, directory) = test_host().await;
    let scope = "conversation-bounded-tool-events";
    let turn = "turn-bounded-tool-events";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context("bounded-user", &agent_id, "agent_user_message", turn, scope),
            json!({"content":"Read the test file"}),
        )
        .await
        .expect("sync user message");
    let task_id = accepted["taskId"].as_str().expect("task ID").to_owned();
    let turn_id = accepted["turnId"].as_str().expect("turn ID").to_owned();
    let session_id = accepted["sessionId"]
        .as_str()
        .expect("session ID")
        .to_owned();
    let marker = "PRIVATE-TOOL-EVENT-MARKER";
    let output = json!({
        "path": "large.txt",
        "content": marker.repeat(50_000),
        "version": "v1:test"
    });
    let mut context = OperationContext::new("bounded-read", &agent_id, "fs_read_text");
    context.task_id = Some(task_id.clone());
    context.turn_id = Some(turn_id);
    context.mcp_session_id = Some(session_id);
    let mut realtime = host.events.subscribe();

    host.append_call_event(
        &context,
        "fs_read_text",
        "succeeded",
        None,
        Some(&output),
        None,
    )
    .await
    .expect("persist bounded event");

    let stored: String = sqlx::query_scalar(
        "SELECT payload_json FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.activityId')='bounded-read' AND json_extract(payload_json,'$.status')='succeeded'",
    )
    .bind(&task_id)
    .fetch_one(host.repository.pool())
    .await
    .expect("read bounded timeline event");
    assert!(!stored.contains(marker));
    assert!(
        stored.len() <= 66 * 1024,
        "stored event was {} bytes",
        stored.len()
    );
    let stored_value: serde_json::Value =
        serde_json::from_str(&stored).expect("bounded event JSON");
    assert_eq!(
        stored_value["payloadExternalized"], true,
        "expected large read output to externalize: {stored}"
    );
    let artifact_id = stored_value["artifactRef"]
        .as_str()
        .expect("managed artifact ref");
    let artifact_path: String =
        sqlx::query_scalar("SELECT relative_path FROM artifact_registry WHERE id=? AND task_id=?")
            .bind(artifact_id)
            .bind(&task_id)
            .fetch_one(host.repository.pool())
            .await
            .expect("managed artifact registry row");
    assert!(artifact_path.starts_with(super::MANAGED_ARTIFACT_PREFIX));
    assert!(!artifact_path.contains(marker));

    let state =
        Arc::new(host.test_app_state(directory.path().join("chatcmd.db").display().to_string()));
    let axum::Json(detail) = crate::api::task_views::task_activity(
        axum::extract::State(state),
        axum::extract::Path((task_id.clone(), "bounded-read".to_owned())),
    )
    .await
    .expect("lazy activity detail");
    assert!(
        detail["output"]["content"]
            .as_str()
            .is_some_and(|content| content.contains(marker)),
        "lazy detail should recover managed artifact output"
    );
    assert_eq!(detail["payloadExternalized"], true);
    assert_eq!(detail["artifactRef"], artifact_id);

    let event = realtime.recv().await.expect("realtime event");
    let encoded = serde_json::to_string(&event).expect("serialize realtime event");
    assert!(!encoded.contains(marker));
    assert!(
        encoded.len() <= 66 * 1024,
        "realtime event was {} bytes",
        encoded.len()
    );
}

#[tokio::test]
async fn externalization_failure_degrades_event_without_losing_tool_status() {
    let (host, agent_id, directory) = test_host().await;
    let scope = "conversation-externalization-degraded";
    let turn = "turn-externalization-degraded";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "degraded-user",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Exercise degraded externalization"}),
        )
        .await
        .expect("sync user message");
    let mut context = OperationContext::new("degraded-read", &agent_id, "fs_read_text");
    context.task_id = accepted["taskId"].as_str().map(ToOwned::to_owned);
    context.turn_id = accepted["turnId"].as_str().map(ToOwned::to_owned);
    context.mcp_session_id = accepted["sessionId"].as_str().map(ToOwned::to_owned);

    std::fs::remove_dir_all(directory.path().join(".test-blobs"))
        .expect("remove blob store root to force artifact failure");
    host.append_call_event(
        &context,
        "fs_read_text",
        "succeeded",
        None,
        Some(&json!({"path":"large.txt","content":"x".repeat(512 * 1024)})),
        None,
    )
    .await
    .expect("bounded event persistence remains available");

    let stored: String = sqlx::query_scalar(
        "SELECT payload_json FROM timeline_events WHERE json_extract(payload_json,'$.activityId')='degraded-read' LIMIT 1",
    )
    .fetch_one(host.repository.pool())
    .await
    .expect("degraded event row");
    let value: serde_json::Value = serde_json::from_str(&stored).expect("event json");
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["externalizationFailed"], true);
    assert!(value["externalizationErrorCode"].is_string());
}

#[tokio::test]
async fn committed_mutation_is_not_rolled_back_when_succeeded_event_database_write_fails() {
    let (host, agent_id, directory) = test_host().await;
    let scope = "conversation-persistence-degraded";
    let turn = "turn-persistence-degraded";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "persistence-user",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Write despite timeline failure"}),
        )
        .await
        .expect("sync user message");

    let target = directory
        .path()
        .join("committed-after-timeline-failure.txt");
    let mut mutation_context = OperationContext::new("degraded-write", &agent_id, "fs_write_text");
    mutation_context.task_id = accepted["taskId"].as_str().map(ToOwned::to_owned);
    mutation_context.turn_id = accepted["turnId"].as_str().map(ToOwned::to_owned);
    mutation_context.mcp_session_id = accepted["sessionId"].as_str().map(ToOwned::to_owned);
    let mutation = host
        .workspace
        .write_text_atomic(
            &mutation_context,
            &target,
            "committed",
            chatcmd_runtime::AtomicWriteOptions::default(),
        )
        .await
        .expect("commit mutation before persistence fault");

    sqlx::query(
        "CREATE TRIGGER fail_succeeded_tool_event BEFORE INSERT ON timeline_events WHEN json_extract(NEW.payload_json,'$.status')='succeeded' BEGIN SELECT RAISE(ABORT,'forced succeeded event failure'); END",
    )
    .execute(host.repository.pool())
    .await
    .expect("install persistence fault trigger");
    let error = host
        .append_call_event(
            &mutation_context,
            "fs_write_text",
            "succeeded",
            None,
            Some(&serde_json::to_value(mutation).expect("mutation json")),
            None,
        )
        .await
        .expect_err("forced succeeded-event persistence failure");
    assert_eq!(error.code, "storage_error");
    assert_eq!(
        std::fs::read_to_string(&target).expect("committed target"),
        "committed",
        "timeline persistence failure must not roll back a committed mutation"
    );
}

#[tokio::test]
#[ignore = "manual Plan 13 contentRef timeline/realtime size benchmark"]
async fn plan13_content_ref_timeline_growth_stays_near_summary_size() {
    let (host, agent_id, _directory) = test_host().await;
    let scope = "conversation-plan13-benchmark";
    let turn = "turn-plan13-benchmark";
    let accepted = host
        .call_persisted(
            "agent_user_message",
            turn_context(
                "plan13-bench-user",
                &agent_id,
                "agent_user_message",
                turn,
                scope,
            ),
            json!({"content":"Plan 13 contentRef benchmark"}),
        )
        .await
        .expect("sync user message");
    let task_id = accepted["taskId"].as_str().expect("task id").to_owned();
    let turn_id = accepted["turnId"].as_str().expect("turn id").to_owned();
    let session_id = accepted["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let mut realtime = host.events.subscribe();

    for size_bytes in [1024_u64 * 1024, 100 * 1024 * 1024, 1024 * 1024 * 1024] {
        let activity_id = format!("plan13-bench-{size_bytes}");
        let mut context = OperationContext::new(&activity_id, &agent_id, "fs_read_text");
        context.task_id = Some(task_id.clone());
        context.turn_id = Some(turn_id.clone());
        context.mcp_session_id = Some(session_id.clone());
        let output = json!({
            "path":"large.txt",
            "contentRef": format!("artifact:plan13:{size_bytes}"),
            "sizeBytes": size_bytes,
            "returnedBytes": 0,
            "truncated": true
        });
        let started = std::time::Instant::now();
        host.append_call_event(
            &context,
            "fs_read_text",
            "succeeded",
            None,
            Some(&output),
            None,
        )
        .await
        .expect("persist benchmark event");
        let serialize_elapsed = started.elapsed();
        let stored: String = sqlx::query_scalar(
            "SELECT payload_json FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.activityId')=? LIMIT 1",
        )
        .bind(&task_id)
        .bind(&activity_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("benchmark event row");
        let event = realtime.recv().await.expect("benchmark realtime event");
        let realtime_bytes = serde_json::to_vec(&event)
            .expect("realtime serialize")
            .len();
        println!(
            "PLAN13_BENCH referenced_mib={} sqlite_event_bytes={} realtime_event_bytes={} serialization_us={}",
            size_bytes / (1024 * 1024),
            stored.len(),
            realtime_bytes,
            serialize_elapsed.as_micros()
        );
        assert!(stored.len() < 16 * 1024);
        assert!(realtime_bytes < 16 * 1024);
    }
}

#[path = "user_message_lifecycle_tests.rs"]
mod lifecycle_tests;
