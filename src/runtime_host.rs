mod activity_control;
mod approval;
mod dispatch;
#[cfg(test)]
mod git_tests;
mod identity;
mod inputs;
mod persistence;
mod subagents;
mod user_message;
#[cfg(test)]
mod user_message_tests;

use chatcmd_core::{AgentId, LocalDevice, McpAgentStore as _, Task};
use chatcmd_mcp::RuntimeApi;
use chatcmd_runtime::{
    BoxFuture, DeviceDescriptor, GitService, OperationContext, ProcessService, RuntimeError,
    RuntimeResult, ShellRuntime, SkillService, WorkspaceService,
};
use chatcmd_storage::SqliteRepository;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::websocket::AppEvent;
pub(crate) use activity_control::{ActivityRegistry, StopActivityResult};

#[derive(Clone)]
pub(crate) struct RuntimeHost {
    repository: SqliteRepository,
    device: LocalDevice,
    shell: ShellRuntime,
    workspace: WorkspaceService,
    git: GitService,
    process: ProcessService,
    skills: SkillService,
    events: broadcast::Sender<AppEvent>,
    activities: ActivityRegistry,
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
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        Self {
            repository,
            device,
            shell,
            workspace,
            git,
            process,
            skills,
            events,
            activities: ActivityRegistry::default(),
        }
    }

    pub(crate) fn activity_registry(&self) -> ActivityRegistry {
        self.activities.clone()
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
        Box::pin(async move { self.call_persisted(tool, context, arguments).await })
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

    fn project_folder<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Option<String>>> {
        Box::pin(async move {
            let id = AgentId::new(agent_id).map_err(|error| invalid("agentId", error))?;
            let agent = self.repository.agent(&id).await.map_err(storage_error)?;
            Ok(agent
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
