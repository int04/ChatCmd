mod activity_control;
mod agent_lifecycle;
mod approval;
#[cfg(test)]
mod approval_regression_tests;
mod chatgpt_identity;
#[cfg(test)]
mod command_tests;
mod completion_report;
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
pub(crate) mod plan_prompt_persistence;
mod queued_messages;
mod subagent_concurrency;
mod subagent_contract;
mod subagent_fallback;
#[cfg(test)]
mod subagent_tests;
mod subagents;
mod task_serialization;
mod terminal_lifecycle;
mod tool_event_projection;
mod turn_file_changes;
mod user_message;
#[cfg(test)]
mod user_message_path_tests;
#[cfg(test)]
mod user_message_project_tests;
#[cfg(test)]
pub(crate) mod user_message_tests;

use chatcmd_core::{LocalDevice, TaskId, TaskStore as _};
use chatcmd_mcp::RuntimeApi;
use chatcmd_runtime::{
    BlobStore, BoxFuture, CommandExecutionService, CursorCodec, DeviceDescriptor, GitService,
    IndexFreshness, OperationContext, ProcessService, RuntimeError, RuntimeResult, ShellRuntime,
    SkillService, WorkspaceService,
};
use chatcmd_storage::{PersistedWorkspaceIndex, PersistedWorkspaceIndexEntry, SqliteRepository};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, broadcast};

use crate::websocket::AppEvent;
pub(crate) const MANAGED_ARTIFACT_PREFIX: &str = "managed:v1:";
pub(crate) use activity_control::{ActivityRegistry, StopActivityResult};
pub(crate) use plan_prompt::{
    PlanPromptRegistry, PlanPromptResolution, PlanPromptResolveError, PlanPromptView,
};
pub(super) use task_serialization::task_json;
use turn_file_changes::TurnFileChangeTracker;

#[derive(Clone)]
pub(crate) struct RuntimeHost {
    repository: SqliteRepository,
    device: LocalDevice,
    shell: ShellRuntime,
    workspace: WorkspaceService,
    blob_store: BlobStore,
    git: GitService,
    command: CommandExecutionService,
    process: ProcessService,
    skills: SkillService,
    events: broadcast::Sender<AppEvent>,
    activities: ActivityRegistry,
    plan_prompts: PlanPromptRegistry,
    file_changes: TurnFileChangeTracker,
    subagent_registration_gate: Arc<Mutex<()>>,
    subagent_worker_id: Arc<str>,
    cursor_codec: CursorCodec,
    telemetry: chatcmd_runtime::ToolTelemetryRegistry,
    repository_index_watchers: Arc<std::sync::Mutex<Vec<notify::RecommendedWatcher>>>,
    repository_index_reconcile_started: Arc<AtomicBool>,
}

impl RuntimeHost {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        repository: SqliteRepository,
        device: LocalDevice,
        shell: ShellRuntime,
        workspace: WorkspaceService,
        blob_store: BlobStore,
        git: GitService,
        command: CommandExecutionService,
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
            command,
            process,
            skills,
            events,
            activities: ActivityRegistry::default(),
            plan_prompts: PlanPromptRegistry::default(),
            file_changes: TurnFileChangeTracker::default(),
            subagent_registration_gate: Arc::new(Mutex::new(())),
            subagent_worker_id: Arc::from(format!("boot-{}", uuid::Uuid::new_v4())),
            cursor_codec: CursorCodec::ephemeral(),
            telemetry: chatcmd_runtime::ToolTelemetryRegistry::new(
                std::env::var("CHATCMD_TELEMETRY")
                    .map_or(true, |value| !matches!(value.trim(), "0" | "off" | "false")),
            ),
            repository_index_watchers: Arc::new(std::sync::Mutex::new(Vec::new())),
            repository_index_reconcile_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) async fn mark_repository_index_stale_for_path(
        &self,
        workspace: &WorkspaceService,
        path: &Path,
    ) -> RuntimeResult<()> {
        workspace.mark_index_stale(path);
        for root in workspace.roots() {
            if path.starts_with(root) {
                self.repository
                    .mark_workspace_index_stale(&root.to_string_lossy())
                    .await
                    .map_err(storage_error)?;
            }
        }
        Ok(())
    }

    pub(crate) async fn restore_repository_indexes(&self) -> RuntimeResult<()> {
        let active_roots = self
            .workspace
            .roots()
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.repository
            .cleanup_workspace_indexes(&active_roots)
            .await
            .map_err(storage_error)?;
        for root in self.workspace.roots() {
            let root_text = root.to_string_lossy();
            let persisted = match self.repository.load_workspace_index(&root_text).await {
                Ok(Some(persisted)) => persisted,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(error = ?error, root = %root.display(), "ignoring corrupt/unreadable repository index; direct filesystem fallback remains active");
                    let _ = self.repository.mark_workspace_index_stale(&root_text).await;
                    continue;
                }
            };
            let freshness = match persisted.state.as_str() {
                "fresh" => IndexFreshness::Fresh,
                "stale" => IndexFreshness::Stale,
                _ => IndexFreshness::Unknown,
            };
            if let Err(error) =
                self.workspace
                    .restore_index_snapshot(chatcmd_runtime::RepositoryIndexSnapshot {
                        root: root.clone(),
                        generation: persisted.generation,
                        freshness,
                        indexed_bytes: persisted.indexed_bytes,
                        schema_version: persisted.schema_version,
                        entries: persisted
                            .entries
                            .into_iter()
                            .map(|entry| chatcmd_runtime::RepositoryIndexEntrySnapshot {
                                relative_path_bytes: entry.relative_path_bytes,
                                display_path: entry.display_path,
                                entry_type: entry.entry_type,
                                size_bytes: entry.size_bytes,
                                modified_at_ns: entry.modified_at_ns,
                            })
                            .collect(),
                    })
            {
                tracing::warn!(error = ?error, root = %root.display(), "ignoring incompatible repository index snapshot; direct filesystem fallback remains active");
                let _ = self.repository.mark_workspace_index_stale(&root_text).await;
            }
        }
        Ok(())
    }

    pub(crate) fn start_repository_index_reconcile(&self) {
        use notify::Watcher as _;

        if self
            .repository_index_reconcile_started
            .swap(true, Ordering::AcqRel)
        {
            return;
        }

        for root in self.workspace.roots().to_vec() {
            let watched_root = root.clone();
            let workspace = self.workspace.clone();
            let repository = self.repository.clone();
            let callback_root = root.clone();
            let runtime = tokio::runtime::Handle::current();
            let callback_runtime = runtime.clone();
            let watcher = notify::RecommendedWatcher::new(
                move |event: notify::Result<notify::Event>| {
                    let paths = event
                        .as_ref()
                        .map(|event| event.paths.clone())
                        .unwrap_or_else(|_| vec![callback_root.clone()]);
                    for path in &paths {
                        workspace.mark_index_stale(path);
                    }
                    let repository = repository.clone();
                    let root_text = callback_root.to_string_lossy().into_owned();
                    callback_runtime.spawn(async move {
                        if let Err(error) = repository.mark_workspace_index_stale(&root_text).await {
                            tracing::warn!(error = ?error, root = %root_text, "failed to persist repository index stale state");
                        }
                    });
                },
                notify::Config::default(),
            );
            match watcher {
                Ok(mut watcher) => {
                    if let Err(error) =
                        watcher.watch(&watched_root, notify::RecursiveMode::Recursive)
                    {
                        self.workspace.mark_index_stale(&watched_root);
                        let repository = self.repository.clone();
                        let root_text = watched_root.to_string_lossy().into_owned();
                        runtime.spawn(async move {
                            if let Err(persist_error) =
                                repository.mark_workspace_index_stale(&root_text).await
                            {
                                tracing::warn!(error = ?persist_error, root = %root_text, "failed to persist watcher-failure stale state");
                            }
                        });
                        tracing::warn!(error = ?error, root = %watched_root.display(), "failed to watch repository index root; periodic reconcile remains active");
                    } else if let Ok(mut watchers) = self.repository_index_watchers.lock() {
                        watchers.push(watcher);
                    }
                }
                Err(error) => {
                    self.workspace.mark_index_stale(&watched_root);
                    let repository = self.repository.clone();
                    let root_text = watched_root.to_string_lossy().into_owned();
                    runtime.spawn(async move {
                        if let Err(persist_error) =
                            repository.mark_workspace_index_stale(&root_text).await
                        {
                            tracing::warn!(error = ?persist_error, root = %root_text, "failed to persist watcher-creation stale state");
                        }
                    });
                    tracing::warn!(error = ?error, root = %watched_root.display(), "failed to create repository index watcher; periodic reconcile remains active");
                }
            }

            let host = self.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let status = match host.workspace.index_status(&root) {
                        Ok(status) => status,
                        Err(error) => {
                            tracing::warn!(error = ?error, root = %root.display(), "repository index status check failed");
                            continue;
                        }
                    };
                    if status.available && status.freshness == IndexFreshness::Fresh {
                        continue;
                    }
                    let context = OperationContext::new(
                        format!("reconcile-index-{}", uuid::Uuid::new_v4()),
                        "runtime",
                        "workspace_index_rebuild",
                    );
                    match host.workspace.rebuild_index(&context, &root).await {
                        Ok(_) => {
                            if let Err(error) =
                                host.persist_repository_index(&host.workspace, &root).await
                            {
                                tracing::warn!(error = ?error, root = %root.display(), "failed to persist reconciled repository index");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = ?error, root = %root.display(), "repository index reconcile failed; direct filesystem fallback remains active");
                        }
                    }
                }
            });
        }
    }

    pub(crate) async fn persist_repository_index(
        &self,
        workspace: &WorkspaceService,
        path: &Path,
    ) -> RuntimeResult<()> {
        let Some(snapshot) = workspace.export_index_snapshot(path)? else {
            return Ok(());
        };
        let state = match snapshot.freshness {
            IndexFreshness::Fresh => "fresh",
            IndexFreshness::Stale => "stale",
            IndexFreshness::Unknown => "unknown",
        }
        .to_owned();
        let persisted = PersistedWorkspaceIndex {
            root_path: snapshot.root.to_string_lossy().into_owned(),
            schema_version: snapshot.schema_version,
            generation: snapshot.generation,
            state,
            indexed_bytes: snapshot.indexed_bytes,
            last_error: None,
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| PersistedWorkspaceIndexEntry {
                    relative_path_bytes: entry.relative_path_bytes,
                    display_path: entry.display_path,
                    entry_type: entry.entry_type,
                    size_bytes: entry.size_bytes,
                    modified_at_ns: entry.modified_at_ns,
                })
                .collect(),
        };
        self.repository
            .replace_workspace_index(&persisted)
            .await
            .map_err(storage_error)
    }

    pub(crate) fn activity_registry(&self) -> ActivityRegistry {
        self.activities.clone()
    }

    pub(crate) fn plan_prompt_registry(&self) -> PlanPromptRegistry {
        self.plan_prompts.clone()
    }

    pub(crate) fn telemetry_registry(&self) -> chatcmd_runtime::ToolTelemetryRegistry {
        self.telemetry.clone()
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
            self.telemetry_registry(),
            self.blob_store.clone(),
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

    fn heartbeat_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async move { RuntimeHost::heartbeat_subagent(self, child_task_id).await })
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

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
