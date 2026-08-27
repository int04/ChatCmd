use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, NullEventSink, OperationContext, PolicyDecision,
    PolicyEngine, RuntimeConfig, RuntimeResult, ShellCreateRequest, ShellRuntime,
    ShellWriteRequest, SystemLocalDevice, WorkspaceService,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _context: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn policy() -> PolicyEngine {
    PolicyEngine::new(
        Some(ExecutionPolicy {
            default: PolicyDecision::Allow,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }),
        Arc::new(Approve),
    )
}

fn runtime(root: PathBuf, max_sessions: usize) -> ShellRuntime {
    ShellRuntime::new(
        RuntimeConfig {
            roots: vec![root],
            max_sessions,
            ..RuntimeConfig::default()
        },
        policy(),
        Arc::new(NullEventSink),
    )
}

fn create_request(root: PathBuf, request_id: &str) -> ShellCreateRequest {
    ShellCreateRequest {
        request_id: request_id.to_owned(),
        working_directory: Some(root),
        executable: None,
        arguments: Vec::new(),
        environment: BTreeMap::new(),
        columns: Some(100),
        rows: Some(24),
    }
}

#[test]
fn exactly_one_local_device() {
    let provider = SystemLocalDevice::new("test");
    assert_eq!(provider.list().len(), 1);
    assert_eq!(
        provider.get("local").expect("local device").device_id,
        "local"
    );
}

#[tokio::test]
async fn shell_lifecycle_timeout_duplicate_and_force_close() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 2);
    let context = OperationContext::new("create-1", "agent", "shell_create");
    let request = create_request(directory.path().to_path_buf(), "create-1");
    let created = runtime
        .create(&context, request.clone())
        .await
        .expect("create shell");
    let duplicate = runtime
        .create(&context, request)
        .await
        .expect("duplicate replay");
    assert_eq!(created.session_id, duplicate.session_id);

    let timed_out = runtime
        .wait(&created.session_id, Duration::from_millis(20))
        .await
        .expect("wait");
    assert!(timed_out.wait_timed_out);
    assert_eq!(
        runtime
            .inspect(&created.session_id)
            .await
            .expect("inspect")
            .status,
        "running"
    );

    let command = if cfg!(windows) {
        "Write-Output chatcmd-ready"
    } else {
        "printf 'chatcmd-ready\\n'"
    };
    runtime
        .write(
            &OperationContext::new("write-1", "agent", "shell_write"),
            ShellWriteRequest {
                request_id: "write-1".to_owned(),
                session_id: created.session_id.clone(),
                text: command.to_owned(),
                append_new_line: true,
            },
        )
        .await
        .expect("write shell");
    let output_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = runtime
            .read(&created.session_id, 0, 100)
            .await
            .expect("read shell");
        if output
            .events
            .iter()
            .any(|event| event.data.contains("chatcmd-ready"))
        {
            break;
        }
        assert!(
            Instant::now() < output_deadline,
            "PTY output did not arrive before the deadline: {output:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    runtime
        .close(
            &OperationContext::new("close-1", "agent", "shell_close"),
            &created.session_id,
            true,
        )
        .await
        .expect("force close");
}

#[tokio::test]
async fn cancellation_and_session_backpressure_are_explicit() {
    let directory = tempfile::tempdir().expect("temp directory");
    let runtime = runtime(directory.path().to_path_buf(), 1);
    let cancelled = OperationContext::new("cancelled", "agent", "shell_create");
    cancelled.cancellation.cancel();
    let error = runtime
        .create(
            &cancelled,
            create_request(directory.path().to_path_buf(), "cancelled"),
        )
        .await
        .expect_err("cancelled create");
    assert_eq!(error.code, "cancelled");

    let first = runtime
        .create(
            &OperationContext::new("first", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "first"),
        )
        .await
        .expect("first shell");
    let busy = runtime
        .create(
            &OperationContext::new("second", "agent", "shell_create"),
            create_request(directory.path().to_path_buf(), "second"),
        )
        .await
        .expect_err("session limit");
    assert_eq!(busy.code, "device_busy");
    runtime
        .close(
            &OperationContext::new("close", "agent", "shell_close"),
            &first.session_id,
            true,
        )
        .await
        .expect("close shell");
}

#[tokio::test]
async fn workspace_traversal_is_denied() {
    let directory = tempfile::tempdir().expect("temp directory");
    let workspace =
        WorkspaceService::new(&[directory.path().to_path_buf()], policy()).expect("workspace");
    let outside = directory
        .path()
        .parent()
        .expect("parent")
        .join("outside.txt");
    let error = workspace
        .read_text(&outside, 100)
        .await
        .expect_err("outside path denied");
    assert!(matches!(
        error.code.as_str(),
        "not_found" | "path_outside_allowed_scope"
    ));
}
