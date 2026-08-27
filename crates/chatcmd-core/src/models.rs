use std::{fmt, str::FromStr};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
        pub struct $name(String);

        impl $name {
            /// Preserves an externally supplied identifier losslessly.
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainError::EmptyIdentifier(stringify!($name)));
                }
                Ok(Self(value))
            }

            /// Returns the exact external representation.
            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }

            /// Consumes this identifier without numeric conversion.
            #[must_use]
            pub fn into_string(self) -> String { self.0 }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;
            fn from_str(value: &str) -> Result<Self, Self::Err> { Self::new(value) }
        }
    };
}

string_id!(
    /// Stable identifier for the one local device.
    DeviceId
);
string_id!(
    /// Identifier for an MCP agent profile.
    AgentId
);
string_id!(
    /// External task identifier.
    TaskId
);
string_id!(
    /// External AI turn identifier.
    TurnId
);
string_id!(
    /// Terminal or task-session identifier.
    SessionId
);
string_id!(
    /// Durable event/idempotency identifier.
    EventId
);
string_id!(
    /// Registered artifact identifier.
    ArtifactId
);

/// Domain validation errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    /// Identifier is empty or whitespace-only.
    #[error("{0} cannot be empty")]
    EmptyIdentifier(&'static str),
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(rename_all = "snake_case"))]
        pub enum $name { $($variant),+ }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = EnumParseError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($value => Ok(Self::$variant),)+ _ => Err(EnumParseError {
                    enum_name: stringify!($name), value: value.to_owned()
                }) }
            }
        }
    };
}

/// Failure to parse a persisted enum value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown {enum_name} value: {value}")]
pub struct EnumParseError {
    /// Enum type expected by the mapper.
    pub enum_name: &'static str,
    /// Unknown persisted value.
    pub value: String,
}

string_enum!(ExecutionMode { Approval => "approval", Allow => "allow", Deny => "deny" });
string_enum!(TaskStatus { Pending => "pending", Running => "running", Completed => "completed", Failed => "failed", Stopped => "stopped", Interrupted => "interrupted" });
string_enum!(TerminalSessionStatus { Starting => "starting", Running => "running", Exited => "exited", Failed => "failed", Closed => "closed", Interrupted => "interrupted" });
string_enum!(ActorKind { User => "user", Assistant => "assistant", System => "system", Tool => "tool", Terminal => "terminal" });
string_enum!(EventKind { Message => "message", Progress => "progress", ToolCall => "tool_call", ToolResult => "tool_result", TerminalOutput => "terminal_output", Status => "status", Warning => "warning" });
string_enum!(ApprovalState { Pending => "pending", Approved => "approved", Rejected => "rejected", Expired => "expired", Cancelled => "cancelled" });
string_enum!(ToolCapability { Read => "read", Write => "write", Execute => "execute", Destructive => "destructive", Network => "network" });
string_enum!(MigrationSourceStatus { Imported => "imported", ImportedWithWarnings => "imported_with_warnings", Failed => "failed" });

/// Logical catalog grouping for MCP tools.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct ToolGroup {
    pub id: String,
    pub key: String,
    pub display_name: String,
    pub sort_order: i32,
}

/// Stable local device descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LocalDevice {
    pub id: DeviceId,
    pub installation_id: String,
    pub name: String,
    pub platform: String,
    pub os_version: Option<String>,
    pub architecture: String,
    pub app_version: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Persisted MCP agent profile. Contains only hash metadata, never raw token material.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct McpAgent {
    pub id: AgentId,
    pub name: String,
    pub enabled: bool,
    pub project_folder: Option<String>,
    pub secret_last4: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
}

/// Input for creating an MCP agent and its first secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMcpAgent {
    pub id: Option<AgentId>,
    pub name: String,
    pub enabled: bool,
    pub project_folder: Option<String>,
}

/// Resolved MCP policy for a path-token-authenticated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAgentPolicy {
    pub agent: McpAgent,
    pub allowed_tool_keys: Vec<String>,
}

/// Persisted MCP tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct ToolDefinition {
    pub id: String,
    pub key: String,
    pub group_id: String,
    pub title: String,
    pub description: String,
    pub input_schema_json: String,
    pub capabilities: Vec<ToolCapability>,
    pub enabled: bool,
}

/// Quick-selection catalog preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPreset {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub tool_ids: Vec<String>,
}

/// Agent/tool allowlist relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAllowedTool {
    pub agent_id: AgentId,
    pub tool_id: String,
}

/// Optional terminal-facing agent name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentName {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub sort_order: i32,
}

/// JSON-backed local setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub key: String,
    pub value_json: String,
    pub updated_at_ms: i64,
}

/// Durable task record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub agent_id: Option<AgentId>,
    pub device_id: DeviceId,
    pub conversation_scope_hash: Option<String>,
    pub title: Option<String>,
    pub source: Option<String>,
    pub status: TaskStatus,
    pub active_session_id: Option<SessionId>,
    pub generation: i32,
    pub stopped_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Task-to-terminal generation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSession {
    pub task_id: TaskId,
    pub session_id: SessionId,
    pub generation: i32,
    pub replaced_session_id: Option<SessionId>,
    pub status: TerminalSessionStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Binding that preserves turn continuity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnBinding {
    pub agent_id: AgentId,
    pub device_id: DeviceId,
    pub turn_id: TurnId,
    pub task_id: TaskId,
    pub last_used_at_ms: i64,
}

/// Terminal process metadata. Output lives in bounded event chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    pub id: SessionId,
    pub task_id: Option<TaskId>,
    pub turn_id: Option<TurnId>,
    pub executable: String,
    pub working_directory: String,
    pub columns: i32,
    pub rows: i32,
    pub process_id: Option<i64>,
    pub status: TerminalSessionStatus,
    pub exit_code: Option<i32>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub closed_at_ms: Option<i64>,
}

/// Ordered, bounded terminal output or state chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEventChunk {
    pub session_id: SessionId,
    pub sequence: i64,
    pub event_id: EventId,
    pub task_id: Option<TaskId>,
    pub turn_id: Option<TurnId>,
    pub kind: EventKind,
    pub stream: Option<String>,
    pub payload: Vec<u8>,
    pub payload_encoding: String,
    pub created_at_ms: i64,
}

/// Durable task/tool/timeline event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEvent {
    pub id: EventId,
    pub task_id: TaskId,
    pub turn_id: Option<TurnId>,
    pub session_id: Option<SessionId>,
    pub actor: ActorKind,
    pub kind: EventKind,
    pub idempotency_key: String,
    pub payload_json: String,
    pub metadata_json: Option<String>,
    pub created_at_ms: i64,
}

/// Pending or resolved command/tool approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: String,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub state: ApprovalState,
    pub request_json: String,
    pub decision_json: Option<String>,
    pub created_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

/// Per-task execution policy override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskExecutionMode {
    pub task_id: TaskId,
    pub mode: ExecutionMode,
    pub updated_at_ms: i64,
}

/// File or generated output associated with a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: ArtifactId,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub relative_path: String,
    pub media_type: Option<String>,
    pub size_bytes: i64,
    pub sha256_hex: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Legacy source fingerprint and import result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSource {
    pub source_key: String,
    pub path: String,
    pub fingerprint: String,
    pub status: MigrationSourceStatus,
    pub warning_count: i64,
    pub imported_at_ms: i64,
    pub error: Option<String>,
}

/// Bootstrap/recovery summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapReport {
    pub schema_version: i64,
    pub device: LocalDevice,
    pub interrupted_tasks: u64,
    pub interrupted_sessions: u64,
}

/// Legacy import summary. Warnings are non-fatal and line-specific.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportReport {
    pub imported_sources: u64,
    pub skipped_sources: u64,
    pub imported_events: u64,
    pub warnings: Vec<String>,
}
