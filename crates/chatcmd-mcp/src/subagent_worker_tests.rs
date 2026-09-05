// These tests explicitly release std::sync::Mutex guards before async cleanup; Clippy's
// conservative await-holding-lock analysis does not track all of those drops reliably.
#![allow(clippy::await_holding_lock)]

use super::sanitize_arguments;
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use rmcp::model::SamplingMessage;
use serde_json::{Map, Value, json};

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
        _task_id: Option<&'a str>,
    ) -> chatcmd_runtime::BoxFuture<'a, RuntimeResult<Option<String>>> {
        let value = self.project_folder.clone();
        Box::pin(async move { Ok(value) })
    }

    fn local_device(&self) -> chatcmd_runtime::DeviceDescriptor {
        chatcmd_runtime::DeviceDescriptor {
            device_id: "device-test".to_owned(),
            machine_id: Some("machine-test".to_owned()),
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

    fn request_subagent_fallback<'a>(
        &'a self,
        parent_context: &'a OperationContext,
        registration: &'a Value,
        delegated_prompt: &'a str,
    ) -> chatcmd_runtime::BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.calls.lock().expect("calls").push((
                "request_subagent_fallback".to_owned(),
                parent_context.clone(),
                json!({
                    "registration": registration,
                    "delegatedPrompt": delegated_prompt,
                }),
            ));
            Ok(json!({ "attempt": 1, "maxAttempts": 3 }))
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

include!("subagent_worker_test_cases.rs");
