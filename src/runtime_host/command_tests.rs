use chatcmd_core::{
    AgentId, ExecutionMode, TaskExecutionMode, TaskId, TaskStore as _, ToolCatalogStore as _,
};
use chatcmd_runtime::OperationContext;
use serde_json::json;

use super::{now_ms, user_message_tests};

#[tokio::test]
async fn command_run_wire_preserves_nonzero_exit_as_execution_result() {
    let (host, agent_id, directory) = user_message_tests::test_host().await;
    crate::catalog_seed::seed_catalog(&host.repository)
        .await
        .expect("seed current catalog");
    let command_tool = host
        .repository
        .list_tools()
        .await
        .expect("list tools")
        .into_iter()
        .find(|tool| tool.key == "command_run")
        .expect("command_run catalog entry");
    host.repository
        .set_agent_allowed_tools(
            &AgentId::new(&agent_id).expect("agent ID"),
            &[command_tool.id],
        )
        .await
        .expect("allow command_run");
    let accepted = host
        .call_persisted(
            "agent_user_message",
            user_message_tests::turn_context(
                "command-user",
                &agent_id,
                "agent_user_message",
                "command-turn",
                "command-scope",
            ),
            json!({"content":"run command boundary regression"}),
        )
        .await
        .expect("user message");
    let task_id = TaskId::new(accepted["taskId"].as_str().expect("task ID")).expect("task ID");
    host.repository
        .set_execution_mode(&TaskExecutionMode {
            task_id: task_id.clone(),
            mode: ExecutionMode::Allow,
            updated_at_ms: now_ms(),
        })
        .await
        .expect("allow task execution");
    let mut context = OperationContext::new("command-call", &agent_id, "command_run");
    context.task_id = Some(task_id.as_str().to_owned());
    context.turn_id = accepted["turnId"].as_str().map(str::to_owned);
    context.mcp_session_id = accepted["sessionId"].as_str().map(str::to_owned);

    #[cfg(windows)]
    let (executable, arguments) = (
        "powershell.exe",
        json!([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output PASS; exit 7"
        ]),
    );
    #[cfg(not(windows))]
    let (executable, arguments) = ("/bin/sh", json!(["-c", "printf PASS; exit 7"]));
    let result = host
        .call_persisted(
            "command_run",
            context,
            json!({
                "executable": executable,
                "arguments": arguments,
                "cwd": directory.path(),
                "timeoutMs": 5_000
            }),
        )
        .await
        .expect("execution result");
    assert_eq!(result["terminalState"], "exited");
    assert_eq!(result["exitCode"], 7);
    assert!(
        result["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("PASS"))
    );
    assert!(
        result["executionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

#[tokio::test]
async fn command_run_cannot_spawn_when_c01_mode_denies_execution() {
    let (host, agent_id, directory) = user_message_tests::test_host().await;
    crate::catalog_seed::seed_catalog(&host.repository)
        .await
        .expect("seed current catalog");
    let command_tool = host
        .repository
        .list_tools()
        .await
        .expect("list tools")
        .into_iter()
        .find(|tool| tool.key == "command_run")
        .expect("command_run catalog entry");
    host.repository
        .set_agent_allowed_tools(
            &AgentId::new(&agent_id).expect("agent ID"),
            &[command_tool.id],
        )
        .await
        .expect("allow command_run tool");
    let accepted = host
        .call_persisted(
            "agent_user_message",
            user_message_tests::turn_context(
                "deny-command-user",
                &agent_id,
                "agent_user_message",
                "deny-command-turn",
                "deny-command-scope",
            ),
            json!({"content":"do not run the command"}),
        )
        .await
        .expect("user message");
    let task_id = TaskId::new(accepted["taskId"].as_str().expect("task ID")).expect("task ID");
    host.repository
        .set_execution_mode(&TaskExecutionMode {
            task_id: task_id.clone(),
            mode: ExecutionMode::Deny,
            updated_at_ms: now_ms(),
        })
        .await
        .expect("deny task execution");
    let sentinel = directory.path().join("must-not-run.txt");
    #[cfg(windows)]
    let (executable, arguments) = (
        "powershell.exe",
        json!([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            format!(
                "Set-Content -LiteralPath '{}' -Value bad",
                sentinel.display()
            )
        ]),
    );
    #[cfg(not(windows))]
    let (executable, arguments) = (
        "/bin/sh",
        json!(["-c", format!("touch '{}'", sentinel.display())]),
    );
    let mut context = OperationContext::new("deny-command-call", &agent_id, "command_run");
    context.task_id = Some(task_id.as_str().to_owned());
    context.turn_id = accepted["turnId"].as_str().map(str::to_owned);
    context.mcp_session_id = accepted["sessionId"].as_str().map(str::to_owned);
    let error = host
        .call_persisted(
            "command_run",
            context,
            json!({"executable": executable, "arguments": arguments, "cwd": directory.path()}),
        )
        .await
        .expect_err("C01 denial must happen before spawn");
    assert_eq!(error.code, "policy_denied");
    assert!(!sentinel.exists());
}
