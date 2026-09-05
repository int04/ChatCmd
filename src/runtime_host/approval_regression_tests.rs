use std::path::Path;

use chatcmd_core::{
    AgentId, ExecutionMode, TaskExecutionMode, TaskId, TaskStore as _, ToolCatalogStore as _,
};
use chatcmd_runtime::OperationContext;
use serde_json::json;
use sqlx::Row as _;
use tokio::time::{Duration, timeout};

use super::{RuntimeHost, now_ms, user_message_tests};

async fn allow_only(host: &RuntimeHost, agent_id: &str, tool: &str) {
    let id = host
        .repository
        .list_tools()
        .await
        .expect("tools")
        .into_iter()
        .find(|candidate| candidate.key == tool)
        .expect("seeded tool")
        .id;
    host.repository
        .set_agent_allowed_tools(&AgentId::new(agent_id).expect("agent"), &[id])
        .await
        .expect("allow tool");
}

async fn task_context(host: &RuntimeHost, agent_id: &str, name: &str) -> OperationContext {
    let turn = format!("{name}-turn");
    let accepted = host
        .call_persisted(
            "agent_user_message",
            user_message_tests::turn_context(
                &format!("{name}-user"),
                agent_id,
                "agent_user_message",
                &turn,
                name,
            ),
            json!({"content":"exercise approval boundary"}),
        )
        .await
        .expect("user message");
    let mut context = OperationContext::new(format!("{name}-call"), agent_id, "shell_create");
    context.task_id = accepted["taskId"].as_str().map(str::to_owned);
    context.turn_id = accepted["turnId"].as_str().map(str::to_owned);
    context.mcp_session_id = accepted["sessionId"].as_str().map(str::to_owned);
    context
}

async fn pending(host: &RuntimeHost, id: &str) {
    timeout(Duration::from_secs(1), async {
        loop {
            let state = sqlx::query_scalar::<_, String>("SELECT state FROM approvals WHERE id=?")
                .bind(id)
                .fetch_optional(host.repository.pool())
                .await
                .expect("approval state");
            if state.as_deref() == Some("pending") {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval became pending");
}

fn sentinel_command(path: &Path) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().replace('\'', "''");
        (
            "powershell.exe".into(),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("[IO.File]::WriteAllText('{path}','spawned')"),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        let path = path.to_string_lossy().replace('\'', "'\\''");
        (
            "/bin/sh".into(),
            vec!["-c".into(), format!("printf spawned > '{path}'")],
        )
    }
}

#[tokio::test]
async fn approval_timeout_expires_without_process_or_sentinel() {
    let (host, agent_id, directory) = user_message_tests::test_host().await;
    allow_only(&host, &agent_id, "shell_create").await;
    let context = task_context(&host, &agent_id, "approval-timeout").await;
    let request_id = context.request_id.clone();
    let sentinel = directory.path().join("timeout-sentinel.txt");
    let (executable, arguments) = sentinel_command(&sentinel);
    let error = timeout(Duration::from_secs(4), <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
        &host, "shell_create", context,
        json!({"workingDirectory":directory.path(),"executable":executable,"arguments":arguments}),
    )).await.expect("bounded approval timeout").expect_err("approval must expire");
    assert_eq!(error.code, "approval_timeout");
    let row = sqlx::query("SELECT state,resolved_at_ms FROM approvals WHERE id=?")
        .bind(request_id)
        .fetch_one(host.repository.pool())
        .await
        .expect("expired row");
    assert_eq!(row.get::<String, _>("state"), "expired");
    assert!(row.get::<Option<i64>, _>("resolved_at_ms").is_some());
    assert!(!sentinel.exists());
    assert!(host.shell.list().await.expect("shells").is_empty());
}

#[tokio::test]
async fn policy_revoke_while_approval_pending_wins_dispatch_recheck() {
    let (host, agent_id, directory) = user_message_tests::test_host().await;
    allow_only(&host, &agent_id, "shell_create").await;
    let context = task_context(&host, &agent_id, "approval-revoke").await;
    let request_id = context.request_id.clone();
    let task_id = TaskId::new(context.task_id.as_deref().expect("task")).expect("task id");
    let sentinel = directory.path().join("revoke-sentinel.txt");
    let (executable, arguments) = sentinel_command(&sentinel);
    let call_host = host.clone();
    let workdir = directory.path().to_path_buf();
    let pending_call = tokio::spawn(async move {
        <RuntimeHost as chatcmd_mcp::RuntimeApi>::call(
            &call_host,
            "shell_create",
            context,
            json!({"workingDirectory":workdir,"executable":executable,"arguments":arguments}),
        )
        .await
    });
    pending(&host, &request_id).await;
    let mut tx = host.repository.pool().begin().await.expect("transaction");
    sqlx::query("UPDATE approvals SET state='approved',decision_json='{}',resolved_at_ms=? WHERE id=? AND state='pending'")
        .bind(now_ms()).bind(&request_id).execute(&mut *tx).await.expect("approve");
    sqlx::query("INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,'deny',?) ON CONFLICT(task_id) DO UPDATE SET mode='deny',updated_at_ms=excluded.updated_at_ms")
        .bind(task_id.as_str()).bind(now_ms()).execute(&mut *tx).await.expect("revoke");
    tx.commit().await.expect("commit revoke");
    let error = timeout(Duration::from_secs(2), pending_call)
        .await
        .expect("join timeout")
        .expect("join")
        .expect_err("revocation wins");
    assert_eq!(error.code, "policy_denied");
    assert!(!sentinel.exists());
    assert!(host.shell.list().await.expect("shells").is_empty());
}

#[tokio::test]
async fn nested_child_uses_root_execution_policy() {
    let (host, agent_id, _directory) = user_message_tests::test_host().await;
    let root = task_context(&host, &agent_id, "nested-root").await;
    let root_id = root.task_id.clone().expect("root");
    let child = host
        .register_subagent(&root, "child", "delegate", None)
        .await
        .expect("child");
    let child_id = child["childTaskId"].as_str().expect("child id").to_owned();
    let mut child_context = root.clone();
    child_context.task_id = Some(child_id.clone());
    child_context.turn_id = Some("nested-child-turn".into());
    let grandchild = host
        .register_subagent(&child_context, "grandchild", "delegate", None)
        .await
        .expect("grandchild");
    let grandchild_id = grandchild["childTaskId"].as_str().expect("grandchild id");
    let resolved = host
        .execution_mode_task_id(&TaskId::new(grandchild_id).expect("id"))
        .await
        .expect("root resolution");
    assert_eq!(resolved.as_str(), root_id);
    host.repository
        .set_execution_mode(&TaskExecutionMode {
            task_id: TaskId::new(&root_id).expect("root id"),
            mode: ExecutionMode::Deny,
            updated_at_ms: now_ms(),
        })
        .await
        .expect("deny root");
    let mut execution = child_context;
    execution.task_id = Some(grandchild_id.to_owned());
    let error = host
        .authorize_execution(&execution, "shell_create", &json!({}))
        .await
        .expect_err("root deny applies");
    assert_eq!(error.code, "policy_denied");
}
