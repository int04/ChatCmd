use std::{path::PathBuf, time::Duration};

use chatcmd_core::{
    ActorKind, ArtifactId, ArtifactStore, EventId, EventKind, ExecutionMode, LocalDevice,
    McpAgentStore, SessionId, Task, TaskExecutionMode, TaskId, TaskStatus, TaskStore,
    TerminalEventChunk, TerminalEventStore, TerminalSession, TerminalSessionStatus,
    TimelineEvent as StoredEvent, ToolCatalogStore,
};
use chatcmd_mcp::RuntimeApi;
use chatcmd_runtime::{
    BoxFuture, DeviceDescriptor, GitService, OperationContext, ProcessService, RuntimeError,
    RuntimeResult, ShellCreateRequest, ShellRuntime, ShellSignal, ShellWriteRequest, SkillService,
    WorkspaceService,
};
use chatcmd_storage::SqliteRepository;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct RuntimeHost {
    repository: SqliteRepository,
    device: LocalDevice,
    shell: ShellRuntime,
    workspace: WorkspaceService,
    git: GitService,
    process: ProcessService,
    skills: SkillService,
}

impl RuntimeHost {
    pub(crate) fn new(
        repository: SqliteRepository,
        device: LocalDevice,
        shell: ShellRuntime,
        workspace: WorkspaceService,
        git: GitService,
        process: ProcessService,
        skills: SkillService,
    ) -> Self {
        Self {
            repository,
            device,
            shell,
            workspace,
            git,
            process,
            skills,
        }
    }

    async fn authorize_tool(&self, agent_id: &str, tool: &str) -> RuntimeResult<()> {
        let id = chatcmd_core::AgentId::new(agent_id)
            .map_err(|_| invalid("agentId", "must be a non-empty string"))?;
        let agent = self
            .repository
            .agent(&id)
            .await
            .map_err(storage_error)?
            .filter(|agent| agent.enabled)
            .ok_or_else(|| RuntimeError::new("unauthorized", "agent is disabled or missing"))?;
        let allowed = self
            .repository
            .agent_allowed_tool_ids(&id)
            .await
            .map_err(storage_error)?;
        let tools = self.repository.list_tools().await.map_err(storage_error)?;
        let permitted = tools.iter().any(|candidate| {
            candidate.key == tool && candidate.enabled && allowed.contains(&candidate.id)
        });
        if permitted {
            let _ = agent;
            Ok(())
        } else {
            Err(RuntimeError::new(
                "policy_denied",
                "agent tool allowlist denied this operation",
            ))
        }
    }

    async fn dispatch(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        self.authorize_tool(&context.agent_id, tool).await?;
        match tool {
            "device_list" => value(vec![self.local_device()]),
            "device_get" => {
                let input: DeviceGet = parse(arguments)?;
                if input.device_id != self.device.id.as_str() && input.device_id != "local" {
                    return Err(RuntimeError::new(
                        "device_not_found",
                        "device was not found",
                    ));
                }
                value(self.local_device())
            }
            "shell_create" => {
                let input: ShellCreate = parse(arguments)?;
                let info = self
                    .shell
                    .create(
                        &context,
                        ShellCreateRequest {
                            request_id: context.request_id.clone(),
                            working_directory: input.working_directory,
                            executable: input.executable,
                            arguments: input.arguments,
                            environment: input.environment,
                            columns: input.columns,
                            rows: input.rows,
                        },
                    )
                    .await?;
                self.persist_shell_session(&context, &info).await?;
                value(info)
            }
            "shell_write" => {
                let input: ShellWrite = parse(arguments)?;
                let written = self
                    .shell
                    .write(
                        &context,
                        ShellWriteRequest {
                            request_id: context.request_id.clone(),
                            session_id: input.session_id,
                            text: input.text,
                            append_new_line: input.append_new_line,
                        },
                    )
                    .await?;
                Ok(json!({ "writtenBytes": written }))
            }
            "shell_wait" => {
                let input: ShellWait = parse(arguments)?;
                let result = self
                    .shell
                    .wait(
                        &input.session_id,
                        Duration::from_millis(input.timeout_ms.clamp(1, 300_000)),
                    )
                    .await?;
                if result.completed {
                    self.update_session_status(&input.session_id, "exited", result.exit_code)
                        .await?;
                }
                value(result)
            }
            "shell_read" => {
                let input: ShellRead = parse(arguments)?;
                let result = self
                    .shell
                    .read(
                        &input.session_id,
                        input.after_sequence,
                        input.max_events.clamp(1, 2_000),
                    )
                    .await?;
                self.persist_shell_events(&context, &result).await?;
                value(result)
            }
            "shell_signal" => {
                let input: ShellSignalInput = parse(arguments)?;
                self.shell
                    .signal(&context, &input.session_id, input.signal)
                    .await?;
                Ok(json!({ "accepted": true }))
            }
            "shell_resize" => {
                let input: ShellResize = parse(arguments)?;
                value(
                    self.shell
                        .resize(&input.session_id, input.columns, input.rows)
                        .await?,
                )
            }
            "shell_close" => {
                let input: ShellClose = parse(arguments)?;
                self.shell
                    .close(&context, &input.session_id, input.force)
                    .await?;
                self.update_session_status(&input.session_id, "closed", None)
                    .await?;
                Ok(json!({ "closed": true }))
            }
            "shell_list" => value(self.shell.list().await?),
            "shell_inspect" => {
                let input: SessionInput = parse(arguments)?;
                value(self.shell.inspect(&input.session_id).await?)
            }
            "workspace_roots" => value(self.workspace.roots()),
            "fs_list" => {
                let input: ListInput = parse(arguments)?;
                value(
                    self.workspace
                        .list(&input.path, input.offset, input.limit)
                        .await?,
                )
            }
            "fs_search" => {
                let input: SearchInput = parse(arguments)?;
                value(
                    self.workspace
                        .search(
                            &input.path,
                            &input.query,
                            input.case_sensitive,
                            input.max_results,
                            input.max_file_bytes,
                        )
                        .await?,
                )
            }
            "fs_find" => {
                let input: FindInput = parse(arguments)?;
                value(
                    self.workspace
                        .find(
                            &input.path,
                            &input.pattern,
                            input.max_results,
                            input.max_depth,
                        )
                        .await?,
                )
            }
            "fs_read_text" => {
                let input: ReadInput = parse(arguments)?;
                value(
                    self.workspace
                        .read_text(&input.path, input.max_characters)
                        .await?,
                )
            }
            "fs_write_text" => {
                let input: WriteTextInput = parse(arguments)?;
                value(
                    self.workspace
                        .write_text(&context, &input.path, &input.content, input.overwrite)
                        .await?,
                )
            }
            "fs_write_raw" => {
                let input: WriteRawInput = parse(arguments)?;
                value(
                    self.workspace
                        .write_raw(&context, &input.path, &input.base64, input.overwrite)
                        .await?,
                )
            }
            "fs_stat" => {
                let input: PathInput = parse(arguments)?;
                value(self.workspace.stat(&input.path).await?)
            }
            "fs_create_directory" => {
                let input: PathInput = parse(arguments)?;
                value(self.workspace.create_directory(&input.path).await?)
            }
            "fs_copy" => {
                let input: TransferInput = parse(arguments)?;
                value(
                    self.workspace
                        .copy(&context, &input.source, &input.destination, input.overwrite)
                        .await?,
                )
            }
            "fs_move" => {
                let input: TransferInput = parse(arguments)?;
                value(
                    self.workspace
                        .move_path(&context, &input.source, &input.destination, input.overwrite)
                        .await?,
                )
            }
            "fs_delete" => {
                let input: DeleteInput = parse(arguments)?;
                Ok(json!({
                    "deleted": self.workspace.delete(&context, &input.path, input.recursive).await?
                }))
            }
            "git_status" => {
                let input: CwdInput = parse(arguments)?;
                value(self.git.status(&input.cwd).await?)
            }
            "git_diff" => {
                let input: GitDiff = parse(arguments)?;
                value(
                    self.git
                        .diff(&input.cwd, input.staged, input.path.as_deref())
                        .await?,
                )
            }
            "git_log" => {
                let input: GitLog = parse(arguments)?;
                value(
                    self.git
                        .log(&input.cwd, input.count, input.path.as_deref())
                        .await?,
                )
            }
            "git_branch" => {
                let input: CwdInput = parse(arguments)?;
                value(self.git.branch(&input.cwd).await?)
            }
            "git_show" => {
                let input: GitShow = parse(arguments)?;
                value(
                    self.git
                        .show(&input.cwd, &input.revision, input.path.as_deref())
                        .await?,
                )
            }
            "git_commit" => {
                let input: GitCommit = parse(arguments)?;
                value(
                    self.git
                        .commit(&input.cwd, &input.message, input.all)
                        .await?,
                )
            }
            "process_list" => value(self.process.list().await?),
            "process_inspect" => {
                let input: ProcessInput = parse(arguments)?;
                value(self.process.inspect(input.process_id).await?)
            }
            "process_kill" => {
                let input: ProcessKill = parse(arguments)?;
                self.process
                    .kill(&context, input.process_id, input.entire_tree)
                    .await?;
                Ok(json!({ "killed": true }))
            }
            "skills_list" => value(self.skills.list().await?),
            "skill_read" => {
                let input: SkillInput = parse(arguments)?;
                value(self.skills.read(&input.skill_id).await?)
            }
            "task_get" => {
                let input: TaskInput = parse(arguments)?;
                let id = TaskId::new(input.task_id).map_err(|error| invalid("taskId", error))?;
                Ok(self
                    .repository
                    .task(&id)
                    .await
                    .map_err(storage_error)?
                    .map_or(Value::Null, task_json))
            }
            "task_list" => Ok(Value::Array(
                self.repository
                    .list_tasks(200)
                    .await
                    .map_err(storage_error)?
                    .into_iter()
                    .map(task_json)
                    .collect(),
            )),
            "task_set_execution_mode" => {
                let input: ExecutionModeInput = parse(arguments)?;
                let id = TaskId::new(input.task_id).map_err(|error| invalid("taskId", error))?;
                let mode = match input.mode.as_str() {
                    "approval" => ExecutionMode::Approval,
                    "allow" | "safe" | "unrestricted" => ExecutionMode::Allow,
                    "deny" => ExecutionMode::Deny,
                    _ => return Err(invalid("mode", "must be approval, allow, or deny")),
                };
                self.repository
                    .set_execution_mode(&TaskExecutionMode {
                        task_id: id,
                        mode,
                        updated_at_ms: now_ms(),
                    })
                    .await
                    .map_err(storage_error)?;
                Ok(json!({ "mode": mode.as_str() }))
            }
            "task_artifact_list" => {
                let input: TaskInput = parse(arguments)?;
                let rows = sqlx::query("SELECT id,relative_path,media_type,size_bytes,created_at_ms FROM artifact_registry WHERE task_id=? ORDER BY created_at_ms,id")
                    .bind(input.task_id)
                    .fetch_all(self.repository.pool())
                    .await
                    .map_err(|_| RuntimeError::new("storage_error", "artifact list unavailable"))?;
                use sqlx::Row as _;
                let items = rows
                    .iter()
                    .map(|row| {
                        json!({
                            "id": row.get::<String, _>("id"),
                            "relativePath": row.get::<String, _>("relative_path"),
                            "mediaType": row.get::<Option<String>, _>("media_type"),
                            "sizeBytes": row.get::<i64, _>("size_bytes"),
                            "createdAtMs": row.get::<i64, _>("created_at_ms")
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(Value::Array(items))
            }
            "task_artifact_read" => {
                let input: ArtifactInput = parse(arguments)?;
                let id = ArtifactId::new(input.artifact_id)
                    .map_err(|error| invalid("artifactId", error))?;
                let artifact = self
                    .repository
                    .artifact(&id)
                    .await
                    .map_err(storage_error)?
                    .filter(|artifact| artifact.task_id.as_str() == input.task_id)
                    .ok_or_else(|| {
                        RuntimeError::new("artifact_not_found", "artifact was not found")
                    })?;
                let path = self
                    .workspace
                    .roots()
                    .iter()
                    .map(|root| root.join(&artifact.relative_path))
                    .find(|path| path.is_file())
                    .ok_or_else(|| {
                        RuntimeError::new("artifact_not_found", "artifact file was not found")
                    })?;
                let read = self.workspace.read_text(&path, 200_000).await?;
                Ok(
                    json!({ "artifact": { "id": artifact.id.as_str(), "taskId": artifact.task_id.as_str(), "sessionId": artifact.session_id.map(|id| id.into_string()), "relativePath": artifact.relative_path, "mediaType": artifact.media_type, "sizeBytes": artifact.size_bytes, "sha256Hex": artifact.sha256_hex }, "content": read.content, "truncated": read.truncated }),
                )
            }
            "agent_progress" => {
                let input: ProgressInput = parse(arguments)?;
                self.save_agent_event(
                    &context,
                    &input.task_id,
                    &input.turn_id,
                    "progress",
                    &input.message,
                    input.suggested_title.as_deref(),
                )
                .await
            }
            "agent_turn_complete" => {
                let input: CompleteInput = parse(arguments)?;
                self.save_agent_event(
                    &context,
                    &input.task_id,
                    &input.turn_id,
                    "completed",
                    &input.content,
                    None,
                )
                .await
            }
            _ => Err(RuntimeError::new("tool_not_found", "unknown MCP tool")),
        }
    }

    async fn persist_shell_session(
        &self,
        context: &OperationContext,
        info: &chatcmd_runtime::ShellSessionInfo,
    ) -> RuntimeResult<()> {
        let task_id = if let Some(raw) = &context.task_id {
            let id = TaskId::new(raw).map_err(|error| invalid("taskId", error))?;
            if self
                .repository
                .task(&id)
                .await
                .map_err(storage_error)?
                .is_none()
            {
                let now = now_ms();
                self.repository
                    .upsert_task(&Task {
                        id: id.clone(),
                        agent_id: chatcmd_core::AgentId::new(&context.agent_id).ok(),
                        device_id: self.device.id.clone(),
                        conversation_scope_hash: None,
                        title: None,
                        source: Some("mcp".to_owned()),
                        status: TaskStatus::Running,
                        active_session_id: Some(
                            SessionId::new(&info.session_id)
                                .map_err(|error| invalid("sessionId", error))?,
                        ),
                        generation: 1,
                        stopped_at_ms: None,
                        created_at_ms: now,
                        updated_at_ms: now,
                    })
                    .await
                    .map_err(storage_error)?;
            }
            Some(id)
        } else {
            None
        };
        self.repository
            .upsert_terminal_session(&TerminalSession {
                id: SessionId::new(&info.session_id)
                    .map_err(|error| invalid("sessionId", error))?,
                task_id,
                turn_id: context
                    .turn_id
                    .as_deref()
                    .map(chatcmd_core::TurnId::new)
                    .transpose()
                    .map_err(|error| invalid("turnId", error))?,
                executable: info.executable.clone(),
                working_directory: info.initial_working_directory.display().to_string(),
                columns: i32::from(info.columns),
                rows: i32::from(info.rows),
                process_id: info.process_id.map(i64::from),
                status: TerminalSessionStatus::Running,
                exit_code: info.exit_code,
                created_at_ms: i64::try_from(info.created_at_unix_ms).unwrap_or(i64::MAX),
                updated_at_ms: now_ms(),
                closed_at_ms: None,
            })
            .await
            .map_err(storage_error)
    }

    async fn persist_shell_events(
        &self,
        context: &OperationContext,
        result: &chatcmd_runtime::ShellReadResult,
    ) -> RuntimeResult<()> {
        let session_id =
            SessionId::new(&result.session_id).map_err(|error| invalid("sessionId", error))?;
        let task_id = context
            .task_id
            .as_deref()
            .map(TaskId::new)
            .transpose()
            .map_err(|error| invalid("taskId", error))?;
        let turn_id = context
            .turn_id
            .as_deref()
            .map(chatcmd_core::TurnId::new)
            .transpose()
            .map_err(|error| invalid("turnId", error))?;
        let chunks = result
            .events
            .iter()
            .map(|event| {
                Ok(TerminalEventChunk {
                    session_id: session_id.clone(),
                    sequence: i64::try_from(event.sequence)
                        .map_err(|error| invalid("sequence", error))?,
                    event_id: EventId::new(format!("{}:{}", result.session_id, event.sequence))
                        .map_err(|error| invalid("eventId", error))?,
                    task_id: task_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: EventKind::TerminalOutput,
                    stream: Some(event.stream.clone()),
                    payload: event.data.as_bytes().to_vec(),
                    payload_encoding: "utf-8".to_owned(),
                    created_at_ms: i64::try_from(event.timestamp_unix_ms).unwrap_or(i64::MAX),
                })
            })
            .collect::<RuntimeResult<Vec<_>>>()?;
        self.repository
            .append_terminal_chunks(&chunks)
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    async fn update_session_status(
        &self,
        session_id: &str,
        status: &str,
        exit_code: Option<i32>,
    ) -> RuntimeResult<()> {
        sqlx::query("UPDATE terminal_sessions SET status=?,exit_code=?,updated_at_ms=?,closed_at_ms=CASE WHEN ? IN ('closed','exited') THEN ? ELSE closed_at_ms END WHERE id=?")
            .bind(status)
            .bind(exit_code)
            .bind(now_ms())
            .bind(status)
            .bind(now_ms())
            .bind(session_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "session state could not be persisted"))?;
        Ok(())
    }

    async fn save_agent_event(
        &self,
        context: &OperationContext,
        task_id: &str,
        turn_id: &str,
        status: &str,
        content: &str,
        title: Option<&str>,
    ) -> RuntimeResult<Value> {
        let task_id = TaskId::new(task_id).map_err(|error| invalid("taskId", error))?;
        let now = now_ms();
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let task = Task {
            id: task_id.clone(),
            agent_id: chatcmd_core::AgentId::new(&context.agent_id).ok(),
            device_id: self.device.id.clone(),
            conversation_scope_hash: None,
            title: title
                .map(str::to_owned)
                .or_else(|| current.as_ref().and_then(|task| task.title.clone())),
            source: Some("mcp".to_owned()),
            status: if status == "completed" {
                TaskStatus::Completed
            } else {
                TaskStatus::Running
            },
            active_session_id: current
                .as_ref()
                .and_then(|task| task.active_session_id.clone()),
            generation: current.as_ref().map_or(1, |task| task.generation),
            stopped_at_ms: None,
            created_at_ms: current.as_ref().map_or(now, |task| task.created_at_ms),
            updated_at_ms: now,
        };
        self.repository
            .upsert_task(&task)
            .await
            .map_err(storage_error)?;
        let event = StoredEvent {
            id: EventId::new(Uuid::new_v4().to_string())
                .map_err(|error| invalid("eventId", error))?,
            task_id,
            turn_id: chatcmd_core::TurnId::new(turn_id).ok(),
            session_id: None,
            actor: ActorKind::Assistant,
            kind: if status == "completed" {
                EventKind::Status
            } else {
                EventKind::Progress
            },
            idempotency_key: context.request_id.clone(),
            payload_json: json!({ "status": status, "content": content }).to_string(),
            metadata_json: None,
            created_at_ms: now,
        };
        chatcmd_core::TerminalEventStore::append_timeline_events(&self.repository, &[event])
            .await
            .map_err(storage_error)?;
        Ok(json!({ "accepted": true, "taskId": task.id.as_str(), "status": status }))
    }
}

impl RuntimeApi for RuntimeHost {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move { self.dispatch(tool, context, arguments).await })
    }

    fn local_device(&self) -> DeviceDescriptor {
        DeviceDescriptor {
            device_id: self.device.id.as_str().to_owned(),
            name: self.device.name.clone(),
            platform: self.device.platform.clone(),
            os_version: self.device.os_version.clone().unwrap_or_default(),
            architecture: self.device.architecture.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            online: true,
        }
    }
}

fn parse<T: DeserializeOwned>(value: Value) -> RuntimeResult<T> {
    serde_json::from_value(value)
        .map_err(|error| RuntimeError::new("invalid_arguments", error.to_string()))
}

fn value<T: serde::Serialize>(value: T) -> RuntimeResult<Value> {
    serde_json::to_value(value)
        .map_err(|_| RuntimeError::new("serialization_failed", "result could not be serialized"))
}

fn invalid(field: &str, error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new("invalid_arguments", format!("{field}: {error}"))
}

fn storage_error(error: chatcmd_core::StorageError) -> RuntimeError {
    match error {
        chatcmd_core::StorageError::NotFound(_) => {
            RuntimeError::new("not_found", "record was not found")
        }
        chatcmd_core::StorageError::Conflict(_) => {
            RuntimeError::new("conflict", "record conflicts with existing data")
        }
        _ => RuntimeError::new("storage_error", "local storage operation failed"),
    }
}

fn task_json(task: Task) -> Value {
    json!({
        "id": task.id.as_str(),
        "agentId": task.agent_id.map(|id| id.into_string()),
        "deviceId": task.device_id.as_str(),
        "title": task.title,
        "source": task.source,
        "status": task.status.as_str(),
        "activeSessionId": task.active_session_id.map(|id| id.into_string()),
        "generation": task.generation,
        "createdAtMs": task.created_at_ms,
        "updatedAtMs": task.updated_at_ms
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

macro_rules! input {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct $name { $( $(#[$meta])* $field: $ty, )* }
    };
}

input!(DeviceGet { device_id: String });
input!(SessionInput { session_id: String });
input!(PathInput { path: PathBuf });
input!(CwdInput { cwd: PathBuf });
input!(TaskInput { task_id: String });
input!(SkillInput { skill_id: String });
input!(ProcessInput { process_id: u32 });
input!(ArtifactInput {
    task_id: String,
    artifact_id: String
});
input!(ExecutionModeInput {
    task_id: String,
    mode: String
});
input!(ShellResize {
    session_id: String,
    columns: u16,
    rows: u16
});
input!(TransferInput {
    source: PathBuf,
    destination: PathBuf,
    #[serde(default)]
    overwrite: bool
});
input!(DeleteInput {
    path: PathBuf,
    #[serde(default)]
    recursive: bool
});
input!(GitShow { cwd: PathBuf, revision: String, #[serde(default)] path: Option<String> });
input!(GitCommit {
    cwd: PathBuf,
    message: String,
    #[serde(default)]
    all: bool
});
input!(ProcessKill {
    process_id: u32,
    #[serde(default)]
    entire_tree: bool
});
input!(CompleteInput {
    task_id: String,
    turn_id: String,
    content: String
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellCreate {
    working_directory: Option<PathBuf>,
    executable: Option<PathBuf>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    environment: std::collections::BTreeMap<String, String>,
    columns: Option<u16>,
    rows: Option<u16>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellWrite {
    session_id: String,
    text: String,
    #[serde(default = "default_true")]
    append_new_line: bool,
}
const fn default_true() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellWait {
    session_id: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
const fn default_timeout() -> u64 {
    30_000
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellRead {
    session_id: String,
    #[serde(default)]
    after_sequence: u64,
    #[serde(default = "default_limit")]
    max_events: usize,
}
const fn default_limit() -> usize {
    200
}
input!(ShellSignalInput {
    session_id: String,
    signal: ShellSignal
});
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellClose {
    session_id: String,
    #[serde(default)]
    force: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListInput {
    path: PathBuf,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchInput {
    path: PathBuf,
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default = "default_limit")]
    max_results: usize,
    #[serde(default = "default_file_bytes")]
    max_file_bytes: u64,
}
const fn default_file_bytes() -> u64 {
    1_048_576
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FindInput {
    path: PathBuf,
    pattern: String,
    #[serde(default = "default_limit")]
    max_results: usize,
    #[serde(default = "default_depth")]
    max_depth: usize,
}
const fn default_depth() -> usize {
    32
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadInput {
    path: PathBuf,
    #[serde(default = "default_characters")]
    max_characters: usize,
}
const fn default_characters() -> usize {
    200_000
}
input!(WriteTextInput {
    path: PathBuf,
    content: String,
    #[serde(default)]
    overwrite: bool
});
input!(WriteRawInput {
    path: PathBuf,
    base64: String,
    #[serde(default)]
    overwrite: bool
});
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitDiff {
    cwd: PathBuf,
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    path: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitLog {
    cwd: PathBuf,
    #[serde(default = "default_git_count")]
    count: usize,
    #[serde(default)]
    path: Option<String>,
}
const fn default_git_count() -> usize {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProgressInput {
    task_id: String,
    turn_id: String,
    message: String,
    #[serde(default)]
    suggested_title: Option<String>,
}
