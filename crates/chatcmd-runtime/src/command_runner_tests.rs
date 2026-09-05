use super::*;
use crate::{ApprovalDecision, BoxFuture, ExecutionPolicy, PolicyDecision, PolicyEngine};
use std::sync::Arc;
use tempfile::TempDir;

struct Approve;

impl ApprovalDecision for Approve {
    fn request<'a>(&'a self, _context: &'a PolicyContext) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn service(directory: &TempDir, decision: PolicyDecision) -> CommandExecutionService {
    let policy = ExecutionPolicy {
        default: decision,
        per_agent_tool: BTreeMap::new(),
        per_root: BTreeMap::new(),
    };
    let engine = PolicyEngine::new(Some(policy), Arc::new(Approve));
    let workspace = WorkspaceService::new(&[directory.path().to_owned()], engine.clone())
        .expect("workspace service");
    CommandExecutionService::new(
        workspace,
        Arc::new(engine),
        directory.path().join("artifacts"),
        2,
    )
}

fn context(request_id: &str, task_id: &str, agent_id: &str) -> OperationContext {
    let mut context = OperationContext::new(request_id, agent_id, "command_run");
    context.task_id = Some(task_id.to_owned());
    context
}

fn request(directory: &TempDir, script: &str) -> CommandRunRequest {
    #[cfg(windows)]
    let (executable, arguments) = (
        "powershell.exe".to_owned(),
        vec![
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            script.to_owned(),
        ],
    );
    #[cfg(not(windows))]
    let (executable, arguments) = (
        "/bin/sh".to_owned(),
        vec!["-c".to_owned(), script.to_owned()],
    );
    CommandRunRequest {
        executable,
        arguments,
        cwd: directory.path().to_owned(),
        environment: BTreeMap::new(),
        idempotency_key: None,
        max_stdout_bytes: 4096,
        max_stderr_bytes: 2048,
        max_artifact_bytes: 16 * 1024,
        timeout_ms: 5_000,
        kill_on_output_limit: false,
    }
}

#[tokio::test]
async fn reports_unicode_and_exit_status_without_interpreting_output() {
    let directory = TempDir::new().expect("temporary directory");
    let service = service(&directory, PolicyDecision::Allow);
    #[cfg(windows)]
    let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; [Console]::Out.Write('xin chào ✓'); [Console]::Error.Write('cảnh báo'); exit 0";
    #[cfg(not(windows))]
    let script = "printf 'xin chào ✓'; printf 'cảnh báo' >&2; exit 0";
    let result = service
        .run(&context("r1", "task", "agent"), request(&directory, script))
        .await
        .expect("command result");
    assert_eq!(result.terminal_state, CommandTerminalState::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout.contains("xin chào"));
    assert!(result.stderr.contains("cảnh báo"));
    assert_eq!(result.signal, None);
    assert!(result.finished_at_unix_ms >= result.started_at_unix_ms);

    #[allow(unused_mut)]
    let mut false_pass = request(&directory, "Write-Output 'PASS'; exit 1");
    #[cfg(not(windows))]
    {
        false_pass.arguments[1] = "printf PASS; exit 1".to_owned();
    }
    let failed = service
        .run(&context("r2", "task", "agent"), false_pass)
        .await
        .expect("exit one is a terminal result");
    assert!(failed.stdout.contains("PASS"));
    assert_eq!(failed.terminal_state, CommandTerminalState::Exited);
    assert_eq!(failed.exit_code, Some(1));
}

#[tokio::test]
async fn timeout_and_cancellation_are_distinct_terminal_states() {
    let directory = TempDir::new().expect("temporary directory");
    let service = service(&directory, PolicyDecision::Allow);
    #[cfg(windows)]
    let sleep = "Start-Sleep -Seconds 30";
    #[cfg(not(windows))]
    let sleep = "sleep 30";
    let mut timed = request(&directory, sleep);
    timed.timeout_ms = 75;
    let timeout = service
        .run(&context("timeout", "task", "agent"), timed)
        .await
        .expect("timeout result");
    assert_eq!(timeout.terminal_state, CommandTerminalState::TimedOut);
    assert!(timeout.timed_out);
    assert!(!timeout.cancelled);
    assert!(timeout.elapsed_ms < 5_000);

    let cancelled_context = context("cancel", "task", "agent");
    cancelled_context.cancellation.cancel();
    let cancelled = service
        .run(&cancelled_context, request(&directory, sleep))
        .await
        .expect("cancel result");
    assert_eq!(cancelled.terminal_state, CommandTerminalState::Cancelled);
    assert!(cancelled.cancelled);
    assert!(!cancelled.timed_out);
}

#[tokio::test]
async fn spawn_failure_and_output_flood_remain_bounded() {
    let directory = TempDir::new().expect("temporary directory");
    let service = service(&directory, PolicyDecision::Allow);
    let mut missing = request(&directory, "unused");
    missing.executable = directory
        .path()
        .join("missing-command")
        .display()
        .to_string();
    missing.arguments.clear();
    let failed = service
        .run(&context("missing", "task", "agent"), missing)
        .await
        .expect("spawn failure result");
    assert_eq!(failed.terminal_state, CommandTerminalState::SpawnFailed);
    assert_eq!(failed.exit_code, None);
    assert!(!failed.stderr.is_empty());

    #[cfg(windows)]
    let flood_script = "$s='界' * 10000; [Console]::OutputEncoding=[Text.Encoding]::UTF8; [Console]::Out.Write($s)";
    #[cfg(not(windows))]
    let flood_script = "yes 界 | head -c 30000";
    let mut flood_request = request(&directory, flood_script);
    flood_request.max_stdout_bytes = 1024;
    flood_request.max_artifact_bytes = 4096;
    let flood = service
        .run(&context("flood", "task", "agent"), flood_request)
        .await
        .expect("flood result");
    assert_eq!(flood.exit_code, Some(0));
    assert!(flood.truncated);
    assert!(flood.stdout.len() <= 1024);
    assert!(flood.artifact_bytes <= 4096);
}

#[tokio::test]
async fn idempotency_reuses_one_execution_and_owner_lookup_is_fail_closed() {
    let directory = TempDir::new().expect("temporary directory");
    let service = service(&directory, PolicyDecision::Allow);
    let marker = directory.path().join("once.txt");
    #[cfg(windows)]
    let script = format!(
        "Add-Content -LiteralPath '{}' -Value once",
        marker.display()
    );
    #[cfg(not(windows))]
    let script = format!("printf 'once\\n' >> '{}'", marker.display());
    let mut command = request(&directory, &script);
    command.idempotency_key = Some("same-key".to_owned());
    let owner = context("first", "task-a", "agent-a");
    let first = service
        .run(&owner, command.clone())
        .await
        .expect("first run");
    let second = service
        .run(&owner, command.clone())
        .await
        .expect("reused run");
    assert_eq!(first.execution_id, second.execution_id);
    assert!(!first.reused);
    assert!(second.reused);
    let contents = std::fs::read_to_string(&marker).expect("marker contents");
    assert_eq!(contents.lines().count(), 1);

    command.arguments.push("different".to_owned());
    let conflict = service.run(&owner, command).await.expect_err("conflict");
    assert_eq!(conflict.code, "idempotency_conflict");
    assert_eq!(
        service
            .result(&owner, &first.execution_id)
            .expect("owner result")
            .exit_code,
        Some(0)
    );
    let other = context("other", "task-a", "agent-b");
    assert_eq!(
        service
            .result(&other, &first.execution_id)
            .expect_err("agent isolation")
            .code,
        "execution_not_found"
    );
}

#[tokio::test]
async fn completed_idempotent_result_survives_service_restart() {
    let directory = TempDir::new().expect("temporary directory");
    let marker = directory.path().join("durable-once.txt");
    #[cfg(windows)]
    let script = format!(
        "Add-Content -LiteralPath '{}' -Value once",
        marker.display()
    );
    #[cfg(not(windows))]
    let script = format!("printf 'once\\n' >> '{}'", marker.display());
    let mut command = request(&directory, &script);
    command.idempotency_key = Some("durable-key".to_owned());
    let owner = context("first", "task", "agent");
    let first = service(&directory, PolicyDecision::Allow)
        .run(&owner, command.clone())
        .await
        .expect("first result");

    let restarted = service(&directory, PolicyDecision::Allow);
    let recovered = restarted
        .run(&owner, command)
        .await
        .expect("persisted result");

    assert!(recovered.reused);
    assert_eq!(recovered.execution_id, first.execution_id);
    assert_eq!(recovered.exit_code, Some(0));
    assert_eq!(
        restarted
            .result(
                &context("other", "task", "other-agent"),
                &recovered.execution_id
            )
            .expect_err("restarted owner isolation")
            .code,
        "execution_not_found"
    );
    assert_eq!(
        std::fs::read_to_string(marker)
            .expect("marker")
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn dropped_request_does_not_duplicate_an_in_flight_execution() {
    let directory = TempDir::new().expect("temporary directory");
    let service = service(&directory, PolicyDecision::Allow);
    let marker = directory.path().join("after-disconnect.txt");
    #[cfg(windows)]
    let script = format!(
        "Start-Sleep -Milliseconds 250; Add-Content -LiteralPath '{}' -Value once",
        marker.display()
    );
    #[cfg(not(windows))]
    let script = format!("sleep 0.25; printf 'once\\n' >> '{}'", marker.display());
    let mut command = request(&directory, &script);
    command.idempotency_key = Some("disconnect-key".to_owned());
    let owner = context("disconnected", "task", "agent");
    let first_service = service.clone();
    let first_owner = owner.clone();
    let first_command = command.clone();
    let request_task =
        tokio::spawn(async move { first_service.run(&first_owner, first_command).await });
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    request_task.abort();

    let recovered = service
        .run(&owner, command)
        .await
        .expect("retry inspects original execution");
    assert!(recovered.reused);
    assert_eq!(recovered.exit_code, Some(0));
    let contents = std::fs::read_to_string(marker).expect("marker contents");
    assert_eq!(contents.lines().count(), 1);
}

#[tokio::test]
async fn policy_deny_prevents_spawn_and_protected_environment_is_rejected() {
    let directory = TempDir::new().expect("temporary directory");
    let denied = service(&directory, PolicyDecision::Deny);
    let marker = directory.path().join("must-not-exist.txt");
    #[cfg(windows)]
    let script = format!("Set-Content -LiteralPath '{}' -Value bad", marker.display());
    #[cfg(not(windows))]
    let script = format!("touch '{}'", marker.display());
    let error = denied
        .run(
            &context("deny", "task", "agent"),
            request(&directory, &script),
        )
        .await
        .expect_err("policy denial");
    assert_eq!(error.code, "policy_denied");
    assert!(!marker.exists());

    let allowed = service(&directory, PolicyDecision::Allow);
    let mut protected = request(&directory, "exit 0");
    protected
        .environment
        .insert("PATH".to_owned(), "bad".to_owned());
    let error = allowed
        .run(&context("env", "task", "agent"), protected)
        .await
        .expect_err("protected environment");
    assert_eq!(error.code, "invalid_arguments");
}

#[tokio::test]
async fn source_state_captures_dirty_untracked_inputs_and_command_edits() {
    let directory = TempDir::new().expect("temporary directory");
    std::fs::write(directory.path().join("dirty.rs"), "dirty before")
        .expect("seed dirty tracked-like input");
    std::fs::write(directory.path().join("untracked.txt"), "untracked before")
        .expect("seed untracked input");
    let service = service(&directory, PolicyDecision::Allow);

    let unchanged = service
        .run(
            &context("source-unchanged", "task", "agent"),
            request(&directory, "exit 0"),
        )
        .await
        .expect("unchanged command");
    let unchanged_before = unchanged.source_state_before.expect("before snapshot");
    let unchanged_after = unchanged.source_state_after.expect("after snapshot");
    assert!(unchanged_before.complete);
    assert_eq!(unchanged_before, unchanged_after);

    let changed = directory.path().join("untracked.txt");
    #[cfg(windows)]
    let script = format!(
        "Start-Sleep -Milliseconds 50; Set-Content -LiteralPath '{}' -Value 'untracked after'",
        changed.display()
    );
    #[cfg(not(windows))]
    let script = format!("sleep 0.05; printf after > '{}'", changed.display());
    let edited = service
        .run(
            &context("source-edited", "task", "agent"),
            request(&directory, &script),
        )
        .await
        .expect("editing command");
    let edited_before = edited.source_state_before.expect("before snapshot");
    let edited_after = edited.source_state_after.expect("after snapshot");
    assert!(edited_before.complete && edited_after.complete);
    assert_ne!(edited_before.digest, edited_after.digest);
}
