mod activity_control;
mod agent_lifecycle;
mod approval;
mod chatgpt_identity;
mod dispatch;
mod filesystem_dispatch;
mod finalization_watchdog;
mod git_support;
#[cfg(test)]
mod git_tests;
mod identity;
mod inputs;
mod persistence;
mod plan_prompt;
mod queued_messages;
mod subagent_concurrency;
mod subagent_fallback;
#[cfg(test)]
mod subagent_tests;
mod subagents;
mod terminal_lifecycle;
mod turn_file_changes;
mod user_message;
#[cfg(test)]
mod user_message_path_tests;
#[cfg(test)]
mod user_message_project_tests;
#[cfg(test)]
pub(crate) mod user_message_tests;

use chatcmd_core::{LocalDevice, Task, TaskId, TaskStore as _};
use chatcmd_mcp::RuntimeApi;
use chatcmd_runtime::{
    BlobStore, BoxFuture, CursorCodec, DeviceDescriptor, GitService, OperationContext,
    ProcessService, RuntimeError, RuntimeResult, ShellRuntime, SkillService, WorkspaceService,
};
use chatcmd_storage::SqliteRepository;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

use crate::websocket::AppEvent;
pub(crate) use activity_control::{ActivityRegistry, StopActivityResult};
pub(crate) use plan_prompt::{
    PlanPromptRegistry, PlanPromptResolution, PlanPromptResolveError, PlanPromptView,
};
use turn_file_changes::TurnFileChangeTracker;

#[derive(Clone)]
pub(crate) struct RuntimeHost {
    repository: SqliteRepository,
    device: LocalDevice,
    shell: ShellRuntime,
    workspace: WorkspaceService,
    blob_store: BlobStore,
    git: GitService,
    process: ProcessService,
    skills: SkillService,
    events: broadcast::Sender<AppEvent>,
    activities: ActivityRegistry,
    plan_prompts: PlanPromptRegistry,
    file_changes: TurnFileChangeTracker,
    subagent_registration_gate: Arc<Mutex<()>>,
    cursor_codec: CursorCodec,
}

impl RuntimeHost {
    pub(crate) fn new(
        repository: SqliteRepository,
        device: LocalDevice,
        shell: ShellRuntime,
        workspace: WorkspaceService,
        blob_store: BlobStore,
        git: GitService,
        process: ProcessService,
        skills: SkillService,
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        Self {
            repository,
            device,
            shell,
            workspace,
            blob_store,
            git,
            process,
            skills,
            events,
            activities: ActivityRegistry::default(),
            plan_prompts: PlanPromptRegistry::default(),
            file_changes: TurnFileChangeTracker::default(),
            subagent_registration_gate: Arc::new(Mutex::new(())),
            cursor_codec: CursorCodec::ephemeral(),
        }
    }

    pub(crate) fn activity_registry(&self) -> ActivityRegistry {
        self.activities.clone()
    }

    pub(crate) fn plan_prompt_registry(&self) -> PlanPromptRegistry {
        self.plan_prompts.clone()
    }

    #[cfg(test)]
    pub(crate) fn test_app_state(&self, database_path: String) -> crate::websocket::AppState {
        crate::websocket::AppState::new(
            self.repository.clone(),
            database_path,
            "127.0.0.1".to_owned(),
            0,
            self.device.clone(),
            self.shell.clone(),
            self.skills.clone(),
            self.activity_registry(),
            self.plan_prompt_registry(),
            self.events.clone(),
        )
    }

    pub(super) fn publish_event(
        &self,
        id: String,
        event_type: &str,
        task_id: Option<String>,
        session_id: Option<String>,
        turn_id: Option<String>,
        payload: Value,
    ) {
        let mut event = AppEvent::new(event_type, payload);
        event.id = id;
        event.task_id = task_id;
        event.session_id = session_id;
        event.turn_id = turn_id;
        let _ = self.events.send(event);
    }
}

impl RuntimeApi for RuntimeHost {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            let output = self.call_persisted(tool, context, arguments).await?;
            Ok(output)
        })
    }

    fn local_device(&self) -> DeviceDescriptor {
        DeviceDescriptor {
            device_id: self.device.id.as_str().to_owned(),
            machine_id: self.device.machine_id.clone(),
            name: self.device.name.clone(),
            platform: self.device.platform.clone(),
            os_version: self.device.os_version.clone().unwrap_or_default(),
            architecture: self.device.architecture.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            online: true,
        }
    }

    fn project_folder<'a>(
        &'a self,
        task_id: Option<&'a str>,
    ) -> BoxFuture<'a, RuntimeResult<Option<String>>> {
        Box::pin(async move {
            let Some(task_id) = task_id.filter(|value| !value.trim().is_empty()) else {
                return Ok(None);
            };
            let id = TaskId::new(task_id).map_err(|error| invalid("taskId", error))?;
            let task = self.repository.task(&id).await.map_err(storage_error)?;
            Ok(task
                .and_then(|value| value.project_folder)
                .filter(|value| !value.trim().is_empty()))
        })
    }

    fn fail_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move { self.fail_subagent_worker(child_task_id, message).await })
    }

    fn request_subagent_fallback<'a>(
        &'a self,
        parent_context: &'a OperationContext,
        registration: &'a Value,
        delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.request_subagent_extension_fallback(parent_context, registration, delegated_prompt)
                .await
        })
    }
}

pub(super) fn parse<T: DeserializeOwned>(value: Value) -> RuntimeResult<T> {
    serde_json::from_value(value)
        .map_err(|error| RuntimeError::new("invalid_arguments", error.to_string()))
}

pub(super) fn value<T: serde::Serialize>(value: T) -> RuntimeResult<Value> {
    serde_json::to_value(value)
        .map_err(|_| RuntimeError::new("serialization_failed", "result could not be serialized"))
}

pub(super) fn invalid(field: &str, error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new("invalid_arguments", format!("{field}: {error}"))
}

pub(super) fn storage_error(error: chatcmd_core::StorageError) -> RuntimeError {
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

pub(super) fn task_json(task: Task) -> Value {
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

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
