use super::{resolve_workspace_root, sanitize_arguments};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use rmcp::model::SamplingMessage;
use serde_json::{Map, Value, json};

#[test]
fn workspace_root_parser_accepts_current_object_shape() {
    let value = json!({
        "roots": [
            { "path": "C:/", "name": "C:/" },
            { "path": "D:/", "name": "D:/" }
        ]
    });
    assert_eq!(resolve_workspace_root(&value).as_deref(), Some("C:/"));
}

#[test]
fn workspace_root_parser_keeps_legacy_array_shape() {
    assert_eq!(
        resolve_workspace_root(&json!(["D:/workspace"])).as_deref(),
        Some("D:/workspace")
    );
}

#[test]
fn strips_parent_correlation_from_child_tool_arguments() {
    let mut arguments = Map::from_iter([
        ("path".to_owned(), json!("a.rs")),
        ("taskId".to_owned(), json!("parent")),
        ("turnId".to_owned(), json!("parent-turn")),
    ]);
    sanitize_arguments(&mut arguments);
    assert_eq!(arguments.get("path"), Some(&json!("a.rs")));
    assert!(!arguments.contains_key("taskId"));
    assert!(!arguments.contains_key("turnId"));
}

#[derive(Clone, Default)]
struct FakeRuntime {
    calls: std::sync::Arc<std::sync::Mutex<Vec<(String, OperationContext, Value)>>>,
    fail_workspace_roots: bool,
    project_folder: Option<String>,
}

impl crate::RuntimeApi for FakeRuntime {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> chatcmd_runtime::BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.calls.lock().expect("calls").push((
                tool.to_owned(),
                context.clone(),
                arguments.clone(),
            ));
            match tool {
                "agent_subagent_start" => Ok(json!({
                    "subagentId": "subagent-test",
                    "taskId": "task-parent",
                    "childTaskId": "task-subagent-test",
                    "name": "File Reader",
                    "status": "pending",
                    "delegationMarker": "CMDGPT_SUBAGENT_ID=subagent-test"
                })),
                "fs_read_text" => Ok(json!({ "content": "file contents", "truncated": false })),
                "workspace_roots" if self.fail_workspace_roots => Err(RuntimeError::new(
                    "user_message_sync_required",
                    "simulated startup failure after registration",
                )),
                "workspace_roots" => Ok(json!(["D:/workspace"])),
                "shell_create" => Ok(json!({ "sessionId": "shell-subagent-test" })),
                "shell_write" => Ok(json!({ "writtenBytes": 1 })),
                "shell_wait" => Ok(json!({
                    "sessionId": "shell-subagent-test",
                    "completed": true,
                    "waitTimedOut": false,
                    "exitCode": 0,
                    "lastSequence": 1
                })),
                "shell_read" => Ok(json!({
                    "sessionId": "shell-subagent-test",
                    "events": [{
                        "data": "\ncodex\nRead native.rs successfully.\ntokens used\n1"
                    }]
                })),
                _ => Ok(json!({ "accepted": true })),
            }
        })
    }

    fn project_folder<'a>(
        &'a self,
        _agent_id: &'a str,
    ) -> chatcmd_runtime::BoxFuture<'a, RuntimeResult<Option<String>>> {
        let value = self.project_folder.clone();
        Box::pin(async move { Ok(value) })
    }

    fn local_device(&self) -> chatcmd_runtime::DeviceDescriptor {
        chatcmd_runtime::DeviceDescriptor {
            device_id: "device-test".to_owned(),
            name: "Test".to_owned(),
            platform: "test".to_owned(),
            os_version: String::new(),
            architecture: "x64".to_owned(),
            app_version: "test".to_owned(),
            online: true,
        }
    }

    fn fail_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
        message: &'a str,
    ) -> chatcmd_runtime::BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move {
            let mut context = OperationContext::new("fail", "agent-test", "fail_subagent");
            context.task_id = Some(child_task_id.to_owned());
            self.calls.lock().expect("calls").push((
                "fail_subagent".to_owned(),
                context,
                json!({ "message": message }),
            ));
            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct SamplingClient {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl rmcp::ClientHandler for SamplingClient {
    async fn create_message(
        &self,
        _params: rmcp::model::CreateMessageRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<rmcp::model::CreateMessageResult, rmcp::ErrorData> {
        use std::sync::atomic::Ordering;
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        if index == 0 {
            Ok(rmcp::model::CreateMessageResult::new(
                SamplingMessage::assistant_tool_use(
                    "tool-call-1",
                    "fs_read_text",
                    Map::from_iter([("path".to_owned(), json!("a.rs"))]),
                ),
                "test-model".to_owned(),
            )
            .with_stop_reason(rmcp::model::CreateMessageResult::STOP_REASON_TOOL_USE))
        } else {
            Ok(rmcp::model::CreateMessageResult::new(
                SamplingMessage::assistant_text("Read a.rs successfully."),
                "test-model".to_owned(),
            )
            .with_stop_reason(rmcp::model::CreateMessageResult::STOP_REASON_END_TURN))
        }
    }

    fn get_info(&self) -> rmcp::model::ClientInfo {
        let mut info = rmcp::model::ClientInfo::default();
        info.capabilities = rmcp::model::ClientCapabilities::builder()
            .enable_sampling()
            .enable_sampling_tools()
            .build();
        info
    }
}

#[derive(Clone, Default)]
struct TextSamplingClient {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl rmcp::ClientHandler for TextSamplingClient {
    async fn create_message(
        &self,
        _params: rmcp::model::CreateMessageRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<rmcp::model::CreateMessageResult, rmcp::ErrorData> {
        use std::sync::atomic::Ordering;
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let content = if index == 0 {
            r#"{"action":"tool","name":"fs_read_text","arguments":{"path":"b.rs"}}"#
        } else {
            r#"{"action":"final","content":"Read b.rs successfully."}"#
        };
        Ok(rmcp::model::CreateMessageResult::new(
            SamplingMessage::assistant_text(content),
            "test-model".to_owned(),
        )
        .with_stop_reason(rmcp::model::CreateMessageResult::STOP_REASON_END_TURN))
    }

    fn get_info(&self) -> rmcp::model::ClientInfo {
        let mut info = rmcp::model::ClientInfo::default();
        info.capabilities = rmcp::model::ClientCapabilities::builder()
            .enable_sampling()
            .build();
        info
    }
}

#[derive(Clone, Default)]
struct NoSamplingClient;

impl rmcp::ClientHandler for NoSamplingClient {}

#[derive(Clone, Default)]
struct BlockingSamplingClient {
    started: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

impl rmcp::ClientHandler for BlockingSamplingClient {
    async fn create_message(
        &self,
        _params: rmcp::model::CreateMessageRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<rmcp::model::CreateMessageResult, rmcp::ErrorData> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(rmcp::model::CreateMessageResult::new(
            SamplingMessage::assistant_text(
                r#"{"action":"final","content":"Background child completed."}"#,
            ),
            "test-model".to_owned(),
        )
        .with_stop_reason(rmcp::model::CreateMessageResult::STOP_REASON_END_TURN))
    }

    fn get_info(&self) -> rmcp::model::ClientInfo {
        let mut info = rmcp::model::ClientInfo::default();
        info.capabilities = rmcp::model::ClientCapabilities::builder()
            .enable_sampling()
            .build();
        info
    }
}

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
async fn startup_failure_after_registration_returns_structured_failed_result() {
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
    assert_eq!(structured.get("status"), Some(&json!("failed")));
    assert_eq!(structured.get("dispatchMode"), Some(&json!("failed")));
    assert_eq!(
        structured.pointer("/startupError/code"),
        Some(&json!("user_message_sync_required"))
    );
    let recorded = recorded.lock().expect("recorded");
    let failed = recorded
        .iter()
        .find(|(tool, _, _)| tool == "fail_subagent")
        .expect("registered child is marked failed");
    assert_eq!(failed.1.task_id.as_deref(), Some("task-subagent-test"));
    drop(recorded);

    client.cancel().await.expect("cancel client");
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn no_sampling_prefers_agent_project_folder_for_shell_workdir() {
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
async fn no_sampling_client_starts_local_codex_fallback() {
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
    assert_eq!(structured.get("dispatchMode"), Some(&json!("localCodex")));
    assert_eq!(
        structured.get("nativeDelegationRequired"),
        Some(&json!(false))
    );
    assert_eq!(structured.get("status"), Some(&json!("running")));
    assert_eq!(structured.get("executor"), Some(&json!("codex")));

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let completed = recorded
                .lock()
                .expect("recorded")
                .iter()
                .any(|(name, _, _)| name == "agent_turn_complete");
            if completed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("local fallback completes");
    let calls = recorded.lock().expect("recorded");
    let names = calls
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.starts_with(&[
        "agent_subagent_start",
        "agent_user_message",
        "workspace_roots",
        "shell_create",
        "shell_write"
    ]));
    assert!(names.contains(&"shell_wait"));
    assert!(names.contains(&"shell_read"));
    assert_eq!(names.last(), Some(&"agent_turn_complete"));
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
