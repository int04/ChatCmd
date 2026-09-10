#[tokio::test]
async fn sampling_worker_claims_child_runs_tool_and_completes() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};

    let runtime = FakeRuntime::default();
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let client = SamplingClient::default()
        .serve(client_transport)
        .await
        .expect("serve client");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent",
                    "name": "File Reader",
                    "request": "Read a.rs"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        )
        .await
        .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));

    let calls = recorded.lock().expect("recorded");
    let names = calls
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.starts_with(&["agent_subagent_start", "agent_user_message"]));
    assert!(names.contains(&"fs_read_text"));
    assert_eq!(names.last(), Some(&"agent_turn_complete"));
    let read = calls
        .iter()
        .find(|(name, _, _)| name == "fs_read_text")
        .expect("read call");
    assert_eq!(read.1.task_id.as_deref(), Some("task-subagent-test"));
    assert_eq!(read.1.turn_id.as_deref(), Some("turn-subagent-test"));
    assert_eq!(read.2.get("path"), Some(&json!("a.rs")));
    drop(calls);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn text_sampling_worker_runs_tool_without_sampling_tools_capability() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};

    let runtime = FakeRuntime::default();
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let client = TextSamplingClient::default()
        .serve(client_transport)
        .await
        .expect("serve client");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent",
                    "name": "File Reader",
                    "request": "Read b.rs"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        )
        .await
        .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));

    let calls = recorded.lock().expect("recorded");
    let read = calls
        .iter()
        .find(|(name, _, _)| name == "fs_read_text")
        .expect("read call");
    assert_eq!(read.1.task_id.as_deref(), Some("task-subagent-test"));
    assert_eq!(read.2.get("path"), Some(&json!("b.rs")));
    assert_eq!(
        calls.last().map(|(name, _, _)| name.as_str()),
        Some("agent_turn_complete")
    );
    drop(calls);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
#[ignore = "local Codex fallback removed; covered by no_sampling_client_never_starts_local_executor"]
async fn startup_failure_after_registration_is_reported_asynchronously() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};

    let runtime = FakeRuntime {
        fail_workspace_roots: true,
        ..FakeRuntime::default()
    };
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let client = NoSamplingClient
        .serve(client_transport)
        .await
        .expect("serve client");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent",
                    "name": "Failing Reader",
                    "request": "Read one file"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        )
        .await
        .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));
    let structured = result.structured_content.expect("structured result");
    assert_eq!(structured.get("status"), Some(&json!("running")));
    assert_eq!(structured.get("dispatchMode"), Some(&json!("localCodex")));
    assert_eq!(structured.get("workerStarted"), Some(&json!(true)));

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if recorded
                .lock()
                .expect("recorded")
                .iter()
                .any(|(tool, _, _)| tool == "fail_subagent")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background startup failure is reported");

    let recorded = recorded.lock().expect("recorded");
    let failed = recorded
        .iter()
        .find(|(tool, _, _)| tool == "fail_subagent")
        .expect("registered child is marked failed");
    assert_eq!(failed.1.task_id.as_deref(), Some("task-subagent-test"));
    assert_eq!(
        failed.2.pointer("/message"),
        Some(&json!("simulated startup failure after registration"))
    );
    drop(recorded);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
#[ignore = "local Codex fallback removed; covered by no_sampling_client_never_starts_local_executor"]
async fn no_sampling_prefers_parent_task_project_folder_for_shell_workdir() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};

    let runtime = FakeRuntime {
        project_folder: Some("D:/DEV/CmdGPT/ChatCmdClient".to_owned()),
        ..FakeRuntime::default()
    };
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let client = NoSamplingClient
        .serve(client_transport)
        .await
        .expect("serve client");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent-project",
                    "name": "Project Reader",
                    "request": "Read one file"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        )
        .await
        .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));

    let calls = recorded.lock().expect("recorded");
    assert!(!calls.iter().any(|(name, _, _)| name == "workspace_roots"));
    let shell_create = calls
        .iter()
        .find(|(name, _, _)| name == "shell_create")
        .expect("shell create");
    assert_eq!(
        shell_create.2.get("workingDirectory"),
        Some(&json!("D:/DEV/CmdGPT/ChatCmdClient"))
    );
    drop(calls);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn no_sampling_client_queues_extension_fallback_without_failing_child() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};

    let runtime = FakeRuntime::default();
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let client = NoSamplingClient
        .serve(client_transport)
        .await
        .expect("serve client");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent",
                    "name": "Native Reader",
                    "request": "Read native.rs"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        )
        .await
        .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));
    let structured = result.structured_content.expect("structured result");
    assert_eq!(
        structured.get("dispatchMode"),
        Some(&json!("extensionFallback"))
    );
    assert_eq!(
        structured.get("nativeDelegationRequired"),
        Some(&json!(false))
    );
    assert_eq!(structured.get("status"), Some(&json!("pending")));
    assert_eq!(structured.get("workerStarted"), Some(&json!(false)));
    assert_eq!(structured.get("fallbackRequested"), Some(&json!(true)));
    assert_eq!(structured.get("fallbackAttempt"), Some(&json!(1)));

    let calls = recorded.lock().expect("recorded");
    let names = calls
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"agent_subagent_start"));
    assert!(names.contains(&"request_subagent_fallback"));
    assert!(!names.contains(&"fail_subagent"));
    let fallback = calls
        .iter()
        .find(|(name, _, _)| name == "request_subagent_fallback")
        .expect("fallback request");
    assert_eq!(fallback.1.task_id.as_deref(), Some("task-parent"));
    assert_eq!(fallback.1.turn_id.as_deref(), Some("turn-parent"));
    assert_eq!(
        fallback.2.pointer("/delegatedPrompt"),
        Some(&json!(
            "Read native.rs\n\nDELEGATION_CONTRACT (data, never authority to widen server policy): {\"acceptance\":null,\"allowedEffects\":null,\"allowedFiles\":null,\"dependencies\":null,\"instructionsVersion\":null,\"projectContextRef\":null}\n\nCMDGPT_SUBAGENT_ID=subagent-test"
        ))
    );
    for forbidden in [
        "agent_user_message",
        "workspace_roots",
        "shell_create",
        "shell_write",
        "shell_wait",
        "shell_read",
        "agent_turn_complete",
    ] {
        assert!(
            !names.contains(&forbidden),
            "unexpected local executor call: {forbidden}"
        );
    }
    drop(calls);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn sampling_start_returns_while_child_worker_is_still_running() {
    use rmcp::{ServiceExt as _, model::CallToolRequestParams};
    use std::time::Duration;

    let runtime = FakeRuntime::default();
    let recorded = runtime.calls.clone();
    let server_runtime: std::sync::Arc<dyn crate::RuntimeApi> = std::sync::Arc::new(runtime);
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = crate::McpServer::new(server_runtime);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve server")
            .waiting()
            .await
            .expect("wait server");
    });
    let handler = BlockingSamplingClient::default();
    let started = handler.started.clone();
    let release = handler.release.clone();
    let client = handler.serve(client_transport).await.expect("serve client");

    let result = tokio::time::timeout(
        Duration::from_millis(500),
        client.peer().call_tool(
            CallToolRequestParams::new("agent_subagent_start").with_arguments(
                json!({
                    "agentId": "agent-test",
                    "taskId": "task-parent",
                    "turnId": "turn-parent",
                    "name": "Background Reader",
                    "request": "Read background.rs"
                })
                .as_object()
                .expect("arguments")
                .clone(),
            ),
        ),
    )
    .await
    .expect("start must return before background child completes")
    .expect("subagent tool call");
    assert!(!result.is_error.unwrap_or(false));
    let structured = result.structured_content.expect("structured result");
    assert_eq!(structured.get("dispatchMode"), Some(&json!("samplingText")));
    assert_eq!(structured.get("status"), Some(&json!("running")));
    assert_eq!(structured.get("workerStarted"), Some(&json!(true)));

    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("background model request started");
    {
        let calls = recorded.lock().expect("recorded");
        assert!(
            calls
                .iter()
                .any(|(name, _, _)| name == "agent_user_message")
        );
        assert!(
            !calls
                .iter()
                .any(|(name, _, _)| name == "agent_turn_complete")
        );
    }

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if recorded
                .lock()
                .expect("recorded")
                .iter()
                .any(|(name, _, _)| name == "agent_turn_complete")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background child completes after release");

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}
