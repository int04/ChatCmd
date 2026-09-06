use super::*;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ToolUsage>,
}

impl RuntimeError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            approval_required: false,
            phase: None,
            usage: None,
        }
    }

    #[must_use]
    pub fn busy(message: impl Into<String>) -> Self {
        Self {
            code: "device_busy".into(),
            message: message.into(),
            retryable: true,
            approval_required: false,
            phase: None,
            usage: None,
        }
    }

    #[must_use]
    pub fn approval(message: impl Into<String>) -> Self {
        Self {
            code: "approval_required".into(),
            message: message.into(),
            retryable: false,
            approval_required: true,
            phase: None,
            usage: None,
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
    pub max_replay_events: usize,
    pub shell_output_chunk_bytes: usize,
    pub shell_output_max_latency_ms: u64,
    pub max_shell_interactive_input_bytes: usize,
    pub max_shell_paste_input_bytes: usize,
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
            max_replay_events: 2_000,
            shell_output_chunk_bytes: 16 * 1024,
            shell_output_max_latency_ms: 25,
            max_shell_interactive_input_bytes: 64 * 1024,
            max_shell_paste_input_bytes: 256 * 1024,
            max_skill_characters: 200_000,
            default_shell: None,
            user_home: None,
            repository_root: None,
        }
    }
}
