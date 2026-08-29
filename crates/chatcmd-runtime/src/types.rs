use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin, sync::Arc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDescriptor {
    pub device_id: String,
    pub machine_id: Option<String>,
    pub name: String,
    pub platform: String,
    pub os_version: String,
    pub architecture: String,
    pub app_version: String,
    pub online: bool,
}

pub trait LocalDeviceProvider: Send + Sync {
    fn local_device(&self) -> DeviceDescriptor;
}

/// Provider for the single machine hosting this direct runtime.
#[derive(Debug, Clone)]
pub struct SystemLocalDevice {
    descriptor: DeviceDescriptor,
}

impl SystemLocalDevice {
    #[must_use]
    pub fn new(app_version: impl Into<String>) -> Self {
        let name = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "local".to_owned());
        Self {
            descriptor: DeviceDescriptor {
                device_id: "local".to_owned(),
                machine_id: None,
                name,
                platform: std::env::consts::OS.to_owned(),
                os_version: std::env::var("OS").unwrap_or_else(|_| std::env::consts::OS.to_owned()),
                architecture: std::env::consts::ARCH.to_owned(),
                app_version: app_version.into(),
                online: true,
            },
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<DeviceDescriptor> {
        vec![self.descriptor.clone()]
    }

    pub fn get(&self, device_id: &str) -> RuntimeResult<DeviceDescriptor> {
        if device_id == self.descriptor.device_id {
            Ok(self.descriptor.clone())
        } else {
            Err(RuntimeError::new(
                "device_not_found",
                "device was not found",
            ))
        }
    }
}

impl LocalDeviceProvider for SystemLocalDevice {
    fn local_device(&self) -> DeviceDescriptor {
        self.descriptor.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationContext {
    pub request_id: String,
    pub agent_id: String,
    pub tool_name: String,
    pub task_id: Option<String>,
    pub turn_id: Option<String>,
    /// Server-derived logical MCP session. Never accepted from untrusted tool arguments.
    pub mcp_session_id: Option<String>,
    /// Server-derived private conversation scope (for example ChatGPT openai/session).
    pub conversation_scope_id: Option<String>,
    #[serde(skip)]
    pub cancellation: CancellationToken,
}

impl OperationContext {
    #[must_use]
    pub fn new(
        request_id: impl Into<String>,
        agent_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            agent_id: agent_id.into(),
            tool_name: tool_name.into(),
            task_id: None,
            turn_id: None,
            mcp_session_id: None,
            conversation_scope_id: None,
            cancellation: CancellationToken::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub event_type: String,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub turn_id: Option<String>,
    pub tool_name: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: TimelineEvent);
}

#[derive(Default)]
pub struct NullEventSink;
impl EventSink for NullEventSink {
    fn emit(&self, _event: TimelineEvent) {}
}

#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub approval_required: bool,
}

impl RuntimeError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            approval_required: false,
        }
    }

    #[must_use]
    pub fn busy(message: impl Into<String>) -> Self {
        Self {
            code: "device_busy".into(),
            message: message.into(),
            retryable: true,
            approval_required: false,
        }
    }

    #[must_use]
    pub fn approval(message: impl Into<String>) -> Self {
        Self {
            code: "approval_required".into(),
            message: message.into(),
            retryable: false,
            approval_required: true,
        }
    }
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub roots: Vec<PathBuf>,
    pub max_sessions: usize,
    pub max_concurrent_operations: usize,
    pub max_replay_bytes: usize,
    pub max_skill_characters: usize,
    pub default_shell: Option<PathBuf>,
    pub user_home: Option<PathBuf>,
    pub repository_root: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_sessions: 8,
            max_concurrent_operations: 4,
            max_replay_bytes: 2 * 1024 * 1024,
            max_skill_characters: 200_000,
            default_shell: None,
            user_home: None,
            repository_root: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCreateRequest {
    pub request_id: String,
    pub working_directory: Option<PathBuf>,
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub columns: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellSessionInfo {
    pub session_id: String,
    pub status: String,
    pub process_id: Option<u32>,
    pub executable: String,
    pub initial_working_directory: PathBuf,
    pub columns: u16,
    pub rows: u16,
    pub created_at_unix_ms: u128,
    pub exit_code: Option<i32>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellWriteRequest {
    pub request_id: String,
    pub session_id: String,
    pub text: String,
    #[serde(default = "default_true")]
    pub append_new_line: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellSignal {
    #[serde(alias = "SIGINT", alias = "sigint")]
    CtrlC,
    #[serde(alias = "SIGBREAK", alias = "sigbreak")]
    CtrlBreak,
    #[serde(alias = "EOF", alias = "eof")]
    Eof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEvent {
    pub sequence: u64,
    pub timestamp_unix_ms: u128,
    pub event_type: String,
    pub stream: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellReadResult {
    pub session_id: String,
    pub oldest_available_sequence: u64,
    pub latest_available_sequence: u64,
    pub replay_truncated: bool,
    pub events: Vec<ShellEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellWaitResult {
    pub session_id: String,
    pub completed: bool,
    pub wait_timed_out: bool,
    pub exit_code: Option<i32>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub path: PathBuf,
    pub name: String,
    pub entry_type: String,
    pub size: u64,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextReadResult {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub process_id: u32,
    pub name: String,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillReadResult {
    pub id: String,
    pub name: String,
    pub source: String,
    pub instructions: String,
    pub truncated: bool,
}

pub trait TaskRuntime: Send + Sync {
    fn task_get<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn task_list(&self) -> BoxFuture<'_, RuntimeResult<serde_json::Value>>;
    fn set_execution_mode<'a>(
        &'a self,
        task_id: &'a str,
        mode: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn artifact_list<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn artifact_read<'a>(
        &'a self,
        task_id: &'a str,
        artifact_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
}

pub trait ProgressSink: Send + Sync {
    fn progress<'a>(
        &'a self,
        task_id: &'a str,
        turn_id: &'a str,
        message: &'a str,
        suggested_title: Option<&'a str>,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
    fn turn_complete<'a>(
        &'a self,
        task_id: &'a str,
        turn_id: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<serde_json::Value>>;
}

pub type SharedEventSink = Arc<dyn EventSink>;
