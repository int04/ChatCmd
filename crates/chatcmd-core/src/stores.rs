use crate::{
    AgentId, AgentName, Approval, Artifact, ArtifactId, BootstrapReport, ExecutionMode,
    GeneratedSecret, ImportReport, LocalDevice, McpAgent, McpAgentPolicy, MigrationSource,
    NewMcpAgent, SessionId, Setting, Task, TaskExecutionMode, TaskId, TaskSession,
    TerminalEventChunk, TerminalSession, TimelineEvent, ToolDefinition, ToolGroup, ToolPreset,
    TurnBinding,
};

/// Shared storage error contract.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("record conflict: {0}")]
    Conflict(String),
    #[error("invalid stored data: {0}")]
    InvalidData(String),
    #[error("storage is busy")]
    Backpressure,
    #[error("storage writer is closed")]
    WriterClosed,
    #[error("database schema {found} is newer than supported schema {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("storage operation failed: {0}")]
    Backend(String),
}

/// Result of agent creation or rotation. Secret raw material is one-time only.
#[derive(Debug)]
pub struct AgentSecretResult {
    pub agent: McpAgent,
    pub secret: GeneratedSecret,
}

/// Store for the one stable local device.
pub trait LocalDeviceStore: Send + Sync {
    async fn local_device(&self) -> Result<LocalDevice, StorageError>;
}

/// MCP agent CRUD and secret lifecycle.
pub trait McpAgentStore: Send + Sync {
    async fn create_agent(&self, input: NewMcpAgent) -> Result<AgentSecretResult, StorageError>;
    async fn list_agents(&self) -> Result<Vec<McpAgent>, StorageError>;
    async fn agent(&self, id: &AgentId) -> Result<Option<McpAgent>, StorageError>;
    async fn rotate_agent_secret(&self, id: &AgentId) -> Result<AgentSecretResult, StorageError>;
    async fn set_agent_enabled(&self, id: &AgentId, enabled: bool) -> Result<(), StorageError>;
    async fn update_agent(
        &self,
        id: &AgentId,
        input: NewMcpAgent,
    ) -> Result<McpAgent, StorageError>;
    async fn delete_agent(&self, id: &AgentId) -> Result<(), StorageError>;
}

/// Path-token-authenticated policy lookup used by the MCP HTTP boundary.
pub trait PolicyLookup: Send + Sync {
    async fn lookup_policy_by_token(
        &self,
        raw_token: &str,
    ) -> Result<Option<McpAgentPolicy>, StorageError>;
}

/// Catalog, groups, presets, allowlists, and agent display names.
pub trait ToolCatalogStore: Send + Sync {
    async fn replace_catalog(
        &self,
        groups: &[ToolGroup],
        tools: &[ToolDefinition],
        presets: &[ToolPreset],
    ) -> Result<(), StorageError>;
    async fn list_tools(&self) -> Result<Vec<ToolDefinition>, StorageError>;
    async fn list_presets(&self) -> Result<Vec<ToolPreset>, StorageError>;
    async fn agent_allowed_tool_ids(&self, agent_id: &AgentId)
    -> Result<Vec<String>, StorageError>;
    async fn set_agent_allowed_tools(
        &self,
        agent_id: &AgentId,
        tool_ids: &[String],
    ) -> Result<(), StorageError>;
    async fn list_agent_names(&self) -> Result<Vec<AgentName>, StorageError>;
}

/// Task/session/turn and approval persistence.
pub trait TaskStore: Send + Sync {
    async fn upsert_task(&self, task: &Task) -> Result<(), StorageError>;
    async fn task(&self, id: &TaskId) -> Result<Option<Task>, StorageError>;
    async fn list_tasks(&self, limit: u32) -> Result<Vec<Task>, StorageError>;
    async fn upsert_task_session(&self, session: &TaskSession) -> Result<(), StorageError>;
    async fn bind_turn(&self, binding: &TurnBinding) -> Result<(), StorageError>;
    async fn save_approval(&self, approval: &Approval) -> Result<(), StorageError>;
    async fn set_execution_mode(&self, mode: &TaskExecutionMode) -> Result<(), StorageError>;
}

/// Ordered terminal and timeline event persistence.
pub trait TerminalEventStore: Send + Sync {
    async fn upsert_terminal_session(&self, session: &TerminalSession) -> Result<(), StorageError>;
    async fn append_terminal_chunks(
        &self,
        chunks: &[TerminalEventChunk],
    ) -> Result<usize, StorageError>;
    async fn terminal_chunks(
        &self,
        session_id: &SessionId,
        after_sequence: Option<i64>,
        limit: u32,
    ) -> Result<Vec<TerminalEventChunk>, StorageError>;
    async fn append_timeline_events(&self, events: &[TimelineEvent])
    -> Result<usize, StorageError>;
}

/// JSON-backed local settings.
pub trait SettingsStore: Send + Sync {
    async fn setting(&self, key: &str) -> Result<Option<Setting>, StorageError>;
    async fn set_setting(&self, setting: &Setting) -> Result<(), StorageError>;
    async fn execution_mode(&self, task_id: Option<&TaskId>)
    -> Result<ExecutionMode, StorageError>;
}

/// Artifact registry; artifact bytes remain in user-controlled files.
pub trait ArtifactStore: Send + Sync {
    async fn register_artifact(&self, artifact: &Artifact) -> Result<(), StorageError>;
    async fn artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>, StorageError>;
}

/// Startup migration, seeding, and stale-session recovery.
pub trait Bootstrap: Send + Sync {
    async fn bootstrap(&self) -> Result<BootstrapReport, StorageError>;
}

/// Explicit crash/restart recovery.
pub trait Recovery: Send + Sync {
    async fn recover_interrupted(&self) -> Result<(u64, u64), StorageError>;
}

/// Read-only legacy import; source files are never changed or deleted.
pub trait LegacyImport: Send + Sync {
    async fn import_legacy(&self, root: &std::path::Path) -> Result<ImportReport, StorageError>;
    async fn migration_sources(&self) -> Result<Vec<MigrationSource>, StorageError>;
}
