fn validate_request(request: &CommandRunRequest) -> RuntimeResult<()> {
    if request.executable.trim().is_empty() || request.executable.contains('\0') {
        return Err(invalid(
            "executable must be non-empty and contain no NUL byte",
        ));
    }
    if request.arguments.len() > MAX_ARGUMENTS
        || request.arguments.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
        || request.arguments.iter().any(|value| value.contains('\0'))
    {
        return Err(invalid(
            "command arguments exceed count/byte limits or contain NUL",
        ));
    }
    if request.environment.len() > MAX_ENVIRONMENT_ENTRIES
        || request
            .environment
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()))
            .sum::<usize>()
            > MAX_ENVIRONMENT_BYTES
        || request
            .environment
            .iter()
            .any(|(key, value)| invalid_environment(key, value))
    {
        return Err(invalid(
            "environment overrides are invalid, protected, or exceed limits",
        ));
    }
    if request.idempotency_key.as_ref().is_some_and(|key| {
        key.trim().is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES || key.contains('\0')
    }) {
        return Err(invalid("idempotencyKey must be 1..200 bytes without NUL"));
    }
    if request.timeout_ms == 0 {
        return Err(invalid("timeoutMs must be greater than zero"));
    }
    Ok(())
}

fn invalid_environment(key: &str, value: &str) -> bool {
    if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
        return true;
    }
    let key = key.to_ascii_uppercase();
    key == "PATH"
        || key == "HOME"
        || key == "USERPROFILE"
        || key == "CODEX_HOME"
        || key == "LD_PRELOAD"
        || key.starts_with("DYLD_")
        || key.starts_with("CHATCMD_")
}

fn request_digest(request: &CommandRunRequest) -> RuntimeResult<String> {
    let bytes = serde_json::to_vec(request).map_err(|_| {
        RuntimeError::new(
            "serialization_failed",
            "command request could not be encoded",
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn command_identity(request: &CommandRunRequest) -> CommandIdentity {
    let mut hasher = Sha256::new();
    for argument in &request.arguments {
        hasher.update(argument.as_bytes());
        hasher.update([0]);
    }
    CommandIdentity {
        executable: request.executable.clone(),
        argument_count: request.arguments.len(),
        arguments_sha256: format!("sha256:{:x}", hasher.finalize()),
    }
}

fn completed_result(
    execution_id: String,
    command: CommandIdentity,
    cwd: PathBuf,
    started_at_unix_ms: u64,
    detailed: BoundedProcessResult,
) -> CommandExecutionResult {
    let output = detailed.output;
    let terminal_state = if output.cancelled {
        CommandTerminalState::Cancelled
    } else if output.timed_out {
        CommandTerminalState::TimedOut
    } else if output.truncation_reason.as_deref() == Some("outputLimit") {
        CommandTerminalState::OutputLimit
    } else if detailed.signal.is_some() {
        CommandTerminalState::Signaled
    } else {
        CommandTerminalState::Exited
    };
    CommandExecutionResult {
        execution_id,
        terminal_state,
        command,
        cwd,
        exit_code: output.exit_code,
        signal: detailed.signal,
        timed_out: output.timed_out,
        cancelled: output.cancelled,
        started_at_unix_ms,
        finished_at_unix_ms: now_unix_ms(),
        elapsed_ms: output.elapsed_ms,
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_bytes: output.stdout_bytes,
        stderr_bytes: output.stderr_bytes,
        truncated: output.truncated,
        truncation_reason: output.truncation_reason,
        artifact_ref: output.artifact_ref,
        artifact_bytes: output.artifact_bytes,
        artifact_sha256: output.artifact_sha256,
        source_state_before: None,
        source_state_after: None,
        reused: false,
    }
}

fn cancelled_result(
    execution_id: String,
    command: CommandIdentity,
    cwd: PathBuf,
    started_at_unix_ms: u64,
) -> CommandExecutionResult {
    empty_result(
        execution_id,
        command,
        cwd,
        started_at_unix_ms,
        CommandTerminalState::Cancelled,
        "command was cancelled before spawn",
    )
}

fn spawn_failed_result(
    execution_id: String,
    command: CommandIdentity,
    cwd: PathBuf,
    started_at_unix_ms: u64,
    error: RuntimeError,
) -> CommandExecutionResult {
    empty_result(
        execution_id,
        command,
        cwd,
        started_at_unix_ms,
        CommandTerminalState::SpawnFailed,
        &error.message,
    )
}

fn empty_result(
    execution_id: String,
    command: CommandIdentity,
    cwd: PathBuf,
    started_at_unix_ms: u64,
    terminal_state: CommandTerminalState,
    message: &str,
) -> CommandExecutionResult {
    CommandExecutionResult {
        execution_id,
        terminal_state,
        command,
        cwd,
        exit_code: None,
        signal: None,
        timed_out: false,
        cancelled: terminal_state == CommandTerminalState::Cancelled,
        started_at_unix_ms,
        finished_at_unix_ms: now_unix_ms(),
        elapsed_ms: 0,
        stdout: String::new(),
        stderr: message.chars().take(1024).collect(),
        stdout_bytes: 0,
        stderr_bytes: u64::try_from(message.len().min(1024)).unwrap_or(u64::MAX),
        truncated: message.len() > 1024,
        truncation_reason: (message.len() > 1024).then(|| "previewLimit".to_owned()),
        artifact_ref: None,
        artifact_bytes: 0,
        artifact_sha256: None,
        source_state_before: None,
        source_state_after: None,
        reused: false,
    }
}

fn required_task_id(context: &OperationContext) -> RuntimeResult<&str> {
    context
        .task_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::new("task_id_required", "command execution requires taskId"))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError::new("invalid_arguments", message)
}

const fn default_stdout_bytes() -> usize {
    512 * 1024
}

const fn default_stderr_bytes() -> usize {
    128 * 1024
}

const fn default_artifact_bytes() -> u64 {
    256 * 1024 * 1024
}

const fn default_timeout_ms() -> u64 {
    30_000
}
