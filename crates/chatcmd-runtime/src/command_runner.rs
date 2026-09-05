//! Bounded, non-interactive command execution with owned lifecycle records.

use crate::{
    GitOutputMode, GitRunOptions, OperationContext, PolicyAuthorizer, PolicyContext, RuntimeError,
    RuntimeResult, WorkspaceService,
    command_execution_registry::{Claim, CommandExecutionRegistry, ExecutionKey},
    command_source_state::capture_source_state,
    process_runner::{BoundedProcessResult, BoundedProcessRunner},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::process::Command;

const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 256 * 1024;
const MAX_ENVIRONMENT_ENTRIES: usize = 128;
const MAX_ENVIRONMENT_BYTES: usize = 256 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRunRequest {
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default = "default_stdout_bytes")]
    pub max_stdout_bytes: usize,
    #[serde(default = "default_stderr_bytes")]
    pub max_stderr_bytes: usize,
    #[serde(default = "default_artifact_bytes")]
    pub max_artifact_bytes: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub kill_on_output_limit: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CommandTerminalState {
    Unknown,
    Exited,
    Signaled,
    TimedOut,
    Cancelled,
    OutputLimit,
    SpawnFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandIdentity {
    pub executable: String,
    pub argument_count: usize,
    pub arguments_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionResult {
    pub execution_id: String,
    pub terminal_state: CommandTerminalState,
    pub command: CommandIdentity,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    pub elapsed_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub artifact_ref: Option<String>,
    pub artifact_bytes: u64,
    pub artifact_sha256: Option<String>,
    pub source_state_before: Option<CommandSourceState>,
    pub source_state_after: Option<CommandSourceState>,
    pub reused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandSourceState {
    pub schema_version: u16,
    pub algorithm: String,
    pub digest: String,
    pub scope: String,
    pub files_scanned: u64,
    pub bytes_hashed: u64,
    pub complete: bool,
    pub limitation: Option<String>,
    pub excluded_directories: Vec<String>,
}

/// Captures the current server-owned source-input state for evidence freshness checks.
pub async fn capture_command_source_state(root: PathBuf) -> CommandSourceState {
    capture_source_state(root).await
}

#[derive(Clone)]
pub struct CommandExecutionService {
    workspace: WorkspaceService,
    authorizer: Arc<dyn PolicyAuthorizer>,
    runner: BoundedProcessRunner,
    concurrency: Arc<tokio::sync::Semaphore>,
    registry: CommandExecutionRegistry,
}

impl CommandExecutionService {
    #[must_use]
    pub fn new(
        workspace: WorkspaceService,
        authorizer: Arc<dyn PolicyAuthorizer>,
        artifact_directory: PathBuf,
        max_concurrent: usize,
    ) -> Self {
        let registry = CommandExecutionRegistry::new(&artifact_directory);
        Self {
            workspace,
            authorizer,
            runner: BoundedProcessRunner::new(artifact_directory),
            concurrency: Arc::new(tokio::sync::Semaphore::new(max_concurrent.clamp(1, 32))),
            registry,
        }
    }

    #[must_use]
    pub fn with_workspace(&self, workspace: WorkspaceService) -> Self {
        Self {
            workspace,
            authorizer: self.authorizer.clone(),
            runner: self.runner.clone(),
            concurrency: self.concurrency.clone(),
            registry: self.registry.clone(),
        }
    }

    pub async fn run(
        &self,
        context: &OperationContext,
        mut request: CommandRunRequest,
    ) -> RuntimeResult<CommandExecutionResult> {
        validate_request(&request)?;
        let task_id = required_task_id(context)?;
        let cwd = self.workspace.stat(&request.cwd).await?.path;
        if !cwd.is_dir() {
            return Err(RuntimeError::new(
                "command_cwd_not_directory",
                "command cwd must be an existing directory",
            ));
        }
        request.cwd.clone_from(&cwd);
        self.authorizer
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "command_run".to_owned(),
                root: Some(cwd.clone()),
                destructive: true,
            })
            .await?;

        let key = ExecutionKey {
            task_id: task_id.to_owned(),
            agent_id: context.agent_id.clone(),
            idempotency_key: request
                .idempotency_key
                .clone()
                .unwrap_or_else(|| context.request_id.clone()),
        };
        let digest = request_digest(&request)?;
        let mut launched = false;
        loop {
            match self
                .registry
                .claim(&key, &digest, command_identity(&request), cwd.clone())
                .await?
            {
                Claim::Finished(mut result) => {
                    result.reused = !launched;
                    return Ok(*result);
                }
                Claim::Wait { notify } => {
                    let notified = notify.notified();
                    if let Some(result) = self.registry.finished(&key)? {
                        let mut result = result;
                        result.reused = !launched;
                        return Ok(result);
                    }
                    notified.await;
                }
                Claim::Run { execution_id } => {
                    launched = true;
                    let service = self.clone();
                    let context = context.clone();
                    let request = request.clone();
                    let key = key.clone();
                    let cwd = cwd.clone();
                    tokio::spawn(async move {
                        let result = service.execute(&context, &request, execution_id, cwd).await;
                        if let Err(error) = service.registry.finish(&key, result).await {
                            tracing::error!(
                                code = error.code,
                                "command execution record could not be finalized"
                            );
                        }
                    });
                }
            }
        }
    }

    pub fn result(
        &self,
        context: &OperationContext,
        execution_id: &str,
    ) -> RuntimeResult<CommandExecutionResult> {
        let task_id = required_task_id(context)?;
        self.registry
            .result(task_id, &context.agent_id, execution_id)
    }

    async fn execute(
        &self,
        context: &OperationContext,
        request: &CommandRunRequest,
        execution_id: String,
        cwd: PathBuf,
    ) -> CommandExecutionResult {
        let started_at_unix_ms = now_unix_ms();
        let identity = command_identity(request);
        let permit = tokio::select! {
            permit = self.concurrency.clone().acquire_owned() => permit.ok(),
            () = context.cancellation.cancelled() => None,
        };
        let Some(_permit) = permit else {
            return cancelled_result(execution_id, identity, cwd, started_at_unix_ms);
        };
        let source_state_before = capture_source_state(cwd.clone()).await;
        let mut command = Command::new(&request.executable);
        command
            .args(&request.arguments)
            .current_dir(&cwd)
            .envs(&request.environment);
        let options = GitRunOptions {
            output_mode: GitOutputMode::InlineOrArtifact,
            max_output_bytes: request.max_stdout_bytes,
            max_stderr_bytes: request.max_stderr_bytes,
            timeout_ms: request.timeout_ms,
            max_runtime_ms: request.timeout_ms,
            artifact_max_bytes: request.max_artifact_bytes,
            kill_on_limit: request.kill_on_output_limit,
            ..GitRunOptions::default()
        };
        let mut result = match self
            .runner
            .run_detailed(command, &options, context.cancellation.clone())
            .await
        {
            Ok(output) => completed_result(execution_id, identity, cwd, started_at_unix_ms, output),
            Err(error) => {
                spawn_failed_result(execution_id, identity, cwd, started_at_unix_ms, error)
            }
        };
        result.source_state_before = Some(source_state_before);
        result.source_state_after = Some(capture_source_state(result.cwd.clone()).await);
        result
    }
}

include!("command_runner_support.rs");

#[cfg(test)]
#[path = "command_runner_tests.rs"]
mod tests;
