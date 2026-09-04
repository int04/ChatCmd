use schemars::JsonSchema;
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
    #[serde(default)]
    pub input_kind: ShellInputKind,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ShellInputKind {
    #[default]
    Interactive,
    Paste,
}

const fn default_true() -> bool {
    true
}

const fn is_false(value: &bool) -> bool {
    !*value
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
    pub encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellReadResult {
    pub session_id: String,
    pub oldest_available_sequence: u64,
    pub latest_available_sequence: u64,
    pub replay_truncated: bool,
    pub dropped_bytes: u64,
    pub dropped_events: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub path: PathBuf,
    pub name: String,
    pub entry_type: String,
    pub size: u64,
    pub readonly: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FsConflictPolicy {
    #[default]
    Error,
    Skip,
    Replace,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FsVerifyMode {
    None,
    #[default]
    Metadata,
    Content,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FsDeleteMode {
    #[default]
    Quarantine,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsMutationBudget {
    #[serde(default = "default_fs_mutation_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fs_mutation_max_files")]
    pub max_files: u64,
    #[serde(default = "default_fs_mutation_max_bytes")]
    pub max_bytes_read: u64,
    #[serde(default = "default_fs_mutation_max_bytes")]
    pub max_bytes_written: u64,
}

impl Default for FsMutationBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_mutation_timeout_ms(),
            max_files: default_fs_mutation_max_files(),
            max_bytes_read: default_fs_mutation_max_bytes(),
            max_bytes_written: default_fs_mutation_max_bytes(),
        }
    }
}

const fn default_fs_mutation_timeout_ms() -> u64 {
    300_000
}
const fn default_fs_mutation_max_files() -> u64 {
    1_000_000
}
const fn default_fs_mutation_max_bytes() -> u64 {
    1024 * 1024 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsTransferRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub conflict_policy: FsConflictPolicy,
    #[serde(default = "default_true")]
    pub atomic_publish: bool,
    #[serde(default)]
    pub verify: FsVerifyMode,
    #[serde(default = "default_true")]
    pub preserve_metadata: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub expected_source_version: Option<String>,
    #[serde(default)]
    pub expected_destination_version: Option<String>,
    #[serde(default)]
    pub budget: FsMutationBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsDeleteRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub mode: FsDeleteMode,
    #[serde(default)]
    pub expected_version: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub budget: FsMutationBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsQuarantineRestoreRequest {
    pub quarantine_path: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsQuarantineGcRequest {
    pub path: PathBuf,
    #[serde(default = "default_quarantine_retention_seconds")]
    pub retention_seconds: u64,
    #[serde(default = "default_quarantine_max_total_bytes")]
    pub max_total_bytes: u64,
    #[serde(default = "default_quarantine_max_items")]
    pub max_items: u64,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsQuarantineGcResult {
    pub scanned_items: u64,
    pub removed_items: u64,
    pub bytes_removed: u64,
    pub retained_bytes: u64,
    pub dry_run: bool,
    pub warnings: Vec<String>,
}

const fn default_quarantine_retention_seconds() -> u64 {
    7 * 24 * 60 * 60
}

const fn default_quarantine_max_total_bytes() -> u64 {
    10 * 1024 * 1024 * 1024
}

const fn default_quarantine_max_items() -> u64 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsMutationResult {
    pub operation_id: String,
    pub state: String,
    pub files_processed: u64,
    pub directories_processed: u64,
    pub bytes_copied: u64,
    pub source_removed: bool,
    pub destination_published: bool,
    pub verified: bool,
    pub rollback_attempted: bool,
    pub rollback_completed: bool,
    pub dry_run: bool,
    pub warnings: Vec<String>,
    pub detail_artifact_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VersionStrength {
    #[default]
    Metadata,
    Sampled,
    Content,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsStatBudget {
    #[serde(default = "default_fs_stat_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fs_stat_max_bytes_read")]
    pub max_bytes_read: u64,
}

impl Default for FsStatBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_stat_timeout_ms(),
            max_bytes_read: default_fs_stat_max_bytes_read(),
        }
    }
}

const fn default_fs_stat_timeout_ms() -> u64 {
    5_000
}

const fn default_fs_stat_max_bytes_read() -> u64 {
    128 * 1024 * 1024
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsPermissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsStatRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub version_strength: VersionStrength,
    #[serde(default)]
    pub hash_algorithm: Option<String>,
    #[serde(default)]
    pub budget: FsStatBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsStatResult {
    pub path: PathBuf,
    pub name: String,
    pub entry_type: String,
    /// Compatibility alias retained for existing clients.
    pub size: u64,
    pub size_bytes: u64,
    pub readonly: bool,
    pub modified_at_ns: Option<u64>,
    pub created_at_ns: Option<u64>,
    pub permissions: FsPermissions,
    pub version_token: String,
    pub version_strength: VersionStrength,
    pub content_hash: Option<String>,
    pub hash_algorithm: Option<String>,
    pub symlink: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsBatchStatRequest {
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub version_strength: VersionStrength,
    #[serde(default = "default_batch_stat_max_items")]
    pub max_items: usize,
    #[serde(default)]
    pub budget: FsBatchStatBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchStatBudget {
    #[serde(default = "default_fs_stat_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_batch_stat_max_items")]
    pub max_metadata_calls: usize,
    #[serde(default = "default_fs_stat_max_bytes_read")]
    pub max_bytes_read: u64,
}

impl Default for FsBatchStatBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_stat_timeout_ms(),
            max_metadata_calls: default_batch_stat_max_items(),
            max_bytes_read: default_fs_stat_max_bytes_read(),
        }
    }
}

const fn default_batch_stat_max_items() -> usize {
    500
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchStatItem {
    pub path: PathBuf,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat: Option<FsStatResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BatchItemError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchUsage {
    pub requested: usize,
    pub succeeded: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchStatResult {
    pub items: Vec<FsBatchStatItem>,
    pub usage: FsBatchUsage,
    pub index_used: bool,
    pub index_generation: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FsListSort {
    #[default]
    Filesystem,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FsListMetadata {
    Type,
    Size,
    Readonly,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsListBudget {
    #[serde(default = "default_fs_list_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fs_list_max_entries_scanned")]
    pub max_entries_scanned: u64,
    #[serde(default = "default_fs_list_max_stats")]
    pub max_stats: u64,
}

impl Default for FsListBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_list_timeout_ms(),
            max_entries_scanned: default_fs_list_max_entries_scanned(),
            max_stats: default_fs_list_max_stats(),
        }
    }
}

const fn default_fs_list_timeout_ms() -> u64 {
    5_000
}

const fn default_fs_list_max_entries_scanned() -> u64 {
    10_000
}

const fn default_fs_list_max_stats() -> u64 {
    1_000
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsListRequestV2 {
    pub path: PathBuf,
    #[serde(default = "default_fs_list_limit")]
    pub limit: usize,
    #[serde(default)]
    pub sort: FsListSort,
    #[serde(default)]
    pub metadata: Vec<FsListMetadata>,
    #[serde(default = "default_true")]
    pub include_hidden: bool,
    #[serde(default)]
    pub budget: FsListBudget,
}

const fn default_fs_list_limit() -> usize {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsListItemV2 {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub name_encoding_lossy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsListPageData {
    pub items: Vec<FsListItemV2>,
    pub directory_version: String,
    pub sort: FsListSort,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsListCursorState {
    pub state_id: String,
    pub directory_version: String,
}

#[derive(Debug, Clone)]
pub struct FsListScanPage {
    pub data: FsListPageData,
    pub has_more: bool,
    pub entries_scanned: u64,
    pub metadata_calls: u64,
    pub truncation_reason: Option<crate::TruncationReason>,
    pub warnings: Vec<crate::ToolWarning>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum FindPatternMode {
    #[default]
    Literal,
    Glob,
    Regex,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FindEntryType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SearchMode {
    #[default]
    Literal,
    Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchBudget {
    #[serde(default = "default_fs_search_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fs_search_max_files_scanned")]
    pub max_files_scanned: u64,
    #[serde(default = "default_fs_search_max_bytes_scanned")]
    pub max_bytes_scanned: u64,
    #[serde(default = "default_fs_search_max_output_bytes")]
    pub max_output_bytes: u64,
    #[serde(default = "default_fs_search_max_file_bytes")]
    pub max_file_bytes: u64,
}

impl Default for FsSearchBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_search_timeout_ms(),
            max_files_scanned: default_fs_search_max_files_scanned(),
            max_bytes_scanned: default_fs_search_max_bytes_scanned(),
            max_output_bytes: default_fs_search_max_output_bytes(),
            max_file_bytes: default_fs_search_max_file_bytes(),
        }
    }
}

const fn default_fs_search_timeout_ms() -> u64 {
    15_000
}
const fn default_fs_search_max_files_scanned() -> u64 {
    100_000
}
const fn default_fs_search_max_bytes_scanned() -> u64 {
    512 * 1024 * 1024
}
const fn default_fs_search_max_output_bytes() -> u64 {
    512 * 1024
}
const fn default_fs_search_max_file_bytes() -> u64 {
    64 * 1024 * 1024
}
const fn default_fs_search_limit() -> usize {
    200
}
const fn default_fs_search_matches_per_file() -> usize {
    50
}
const fn default_fs_search_snippet_bytes() -> usize {
    8 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchRequest {
    pub path: PathBuf,
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub word_boundary: bool,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub include_ignored: bool,
    #[serde(default)]
    pub context_before: usize,
    #[serde(default)]
    pub context_after: usize,
    #[serde(default = "default_fs_search_matches_per_file")]
    pub max_matches_per_file: usize,
    #[serde(default = "default_fs_search_limit")]
    pub limit: usize,
    #[serde(default = "default_fs_search_snippet_bytes")]
    pub max_snippet_bytes: usize,
    #[serde(default)]
    pub budget: FsSearchBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchMatch {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub byte_offset: u64,
    pub match_start: u64,
    pub match_end: u64,
    pub match_text: String,
    pub line_text: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    pub line_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchPageData {
    pub matches: Vec<FsSearchMatch>,
    pub files_skipped_by_size: u64,
    pub binary_files_skipped: u64,
    pub errors_skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchCursorState {
    pub state_id: String,
    pub root_version: String,
}

#[derive(Debug, Clone)]
pub struct FsSearchScanPage {
    pub data: FsSearchPageData,
    pub has_more: bool,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub truncation_reason: Option<crate::TruncationReason>,
    pub warnings: Vec<crate::ToolWarning>,
    pub root_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsFindBudget {
    #[serde(default = "default_fs_find_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fs_find_max_entries_scanned")]
    pub max_entries_scanned: u64,
    #[serde(default = "default_fs_find_max_metadata_calls")]
    pub max_metadata_calls: u64,
}

impl Default for FsFindBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_find_timeout_ms(),
            max_entries_scanned: default_fs_find_max_entries_scanned(),
            max_metadata_calls: default_fs_find_max_metadata_calls(),
        }
    }
}

const fn default_fs_find_timeout_ms() -> u64 {
    10_000
}
const fn default_fs_find_max_entries_scanned() -> u64 {
    100_000
}
const fn default_fs_find_max_metadata_calls() -> u64 {
    10_000
}
const fn default_fs_find_limit() -> usize {
    200
}
const fn default_fs_find_max_depth() -> usize {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsFindRequest {
    pub path: PathBuf,
    pub pattern: String,
    #[serde(default)]
    pub pattern_mode: FindPatternMode,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub entry_types: Vec<FindEntryType>,
    #[serde(default = "default_fs_find_max_depth")]
    pub max_depth: usize,
    #[serde(default)]
    pub include_ignored: bool,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default = "default_fs_find_limit")]
    pub limit: usize,
    #[serde(default)]
    pub budget: FsFindBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsFindItem {
    pub path: String,
    pub entry_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsFindPageData {
    pub items: Vec<FsFindItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FsFindCursorState {
    pub state_id: String,
    pub root_version: String,
}

#[derive(Debug, Clone)]
pub struct FsFindScanPage {
    pub data: FsFindPageData,
    pub has_more: bool,
    pub entries_scanned: u64,
    pub metadata_calls: u64,
    pub truncation_reason: Option<crate::TruncationReason>,
    pub warnings: Vec<crate::ToolWarning>,
    pub root_version: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadBudget {
    #[serde(default = "default_text_read_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_text_read_max_bytes_read")]
    pub max_bytes_read: u64,
}

impl Default for TextReadBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_text_read_timeout_ms(),
            max_bytes_read: default_text_read_max_bytes_read(),
        }
    }
}

const fn default_text_read_timeout_ms() -> u64 {
    10_000
}

const fn default_text_read_max_bytes_read() -> u64 {
    8 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", rename_all = "camelCase")]
pub enum TextReadRange {
    Line { start: usize, limit: usize },
    Byte { start: u64, limit: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadRequestV2 {
    pub path: PathBuf,
    pub range: TextReadRange,
    #[serde(default = "default_text_read_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_true")]
    pub include_line_endings: bool,
    #[serde(default)]
    pub expected_version: Option<String>,
    #[serde(default)]
    pub budget: TextReadBudget,
}

const fn default_text_read_max_bytes() -> usize {
    256 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadRangeResult {
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub start_byte: Option<u64>,
    pub end_byte: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextReadResultV2 {
    pub path: PathBuf,
    pub content: String,
    pub range: TextReadRangeResult,
    pub next_start_line: Option<usize>,
    pub next_byte_offset: Option<u64>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub bytes_read: u64,
    pub size_bytes: u64,
    pub version_token: String,
    pub encoding: String,
    pub bom: bool,
    pub line_ending: String,
    pub line_ending_detection: String,
    pub total_lines: Option<usize>,
    pub total_lines_known: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsBatchReadRequest {
    pub requests: Vec<TextReadRequestV2>,
    #[serde(default = "default_batch_read_max_items")]
    pub max_items: usize,
    #[serde(default = "default_batch_read_output_bytes")]
    pub max_total_output_bytes: usize,
    #[serde(default = "default_batch_read_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub budget: TextReadBudget,
}

const fn default_batch_read_max_items() -> usize {
    50
}
const fn default_batch_read_output_bytes() -> usize {
    1024 * 1024
}
const fn default_batch_read_concurrency() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchReadItem {
    pub path: PathBuf,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TextReadResultV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BatchItemError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsBatchReadResult {
    pub items: Vec<FsBatchReadItem>,
    pub usage: FsBatchUsage,
    pub output_bytes: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IndexFreshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIndexStatus {
    pub root: PathBuf,
    pub available: bool,
    pub generation: u64,
    pub freshness: IndexFreshness,
    pub entry_count: usize,
    pub indexed_bytes: u64,
    pub schema_version: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DurabilityMode {
    None,
    #[default]
    Data,
    Full,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataPolicy {
    #[default]
    Preserve,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AtomicWriteOptions {
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
    #[serde(default)]
    pub metadata_policy: MetadataPolicy,
    #[serde(default)]
    pub durability: DurabilityMode,
    #[serde(default)]
    pub require_atomic: bool,
}

impl Default for AtomicWriteOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            expected_version: None,
            metadata_policy: MetadataPolicy::Preserve,
            durability: DurabilityMode::Data,
            require_atomic: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AtomicWriteResult {
    pub path: PathBuf,
    pub committed: bool,
    pub created: bool,
    pub atomic: bool,
    pub durability_requested: DurabilityMode,
    pub durability_achieved: DurabilityMode,
    pub bytes_written: u64,
    pub old_version: Option<String>,
    pub new_version: String,
    pub metadata_preserved: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EditCoordinateSystem {
    #[default]
    Byte,
    LineColumn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EditColumnEncoding {
    Utf8CodePoint,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_byte: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_byte: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<TextPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TextPosition>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyEditsBudget {
    #[serde(default = "default_apply_edits_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_apply_edits_max_bytes_read")]
    pub max_bytes_read: u64,
    #[serde(default = "default_apply_edits_max_bytes_written")]
    pub max_bytes_written: u64,
    #[serde(default = "default_apply_edits_max_edits")]
    pub max_edits: usize,
}

impl Default for ApplyEditsBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_apply_edits_timeout_ms(),
            max_bytes_read: default_apply_edits_max_bytes_read(),
            max_bytes_written: default_apply_edits_max_bytes_written(),
            max_edits: default_apply_edits_max_edits(),
        }
    }
}

const fn default_apply_edits_timeout_ms() -> u64 {
    15_000
}
const fn default_apply_edits_max_bytes_read() -> u64 {
    1024 * 1024 * 1024
}
const fn default_apply_edits_max_bytes_written() -> u64 {
    1024 * 1024 * 1024
}
const fn default_apply_edits_max_edits() -> usize {
    1_000
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyEditsRequest {
    pub path: PathBuf,
    pub expected_version: String,
    pub coordinate_system: EditCoordinateSystem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_encoding: Option<EditColumnEncoding>,
    pub edits: Vec<TextEdit>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_true")]
    pub preserve_line_endings: bool,
    #[serde(default = "default_true")]
    pub preserve_bom: bool,
    #[serde(default)]
    pub budget: ApplyEditsBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyEditsResult {
    pub path: PathBuf,
    pub applied: bool,
    pub dry_run: bool,
    pub old_version: String,
    pub new_version: String,
    pub edits_applied: usize,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub additions: u64,
    pub deletions: u64,
    pub preview: String,
    pub diff_artifact_ref: Option<String>,
    pub commit_state: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GitOutputMode {
    Inline,
    #[default]
    InlineOrArtifact,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GitRunOptions {
    pub output_mode: GitOutputMode,
    pub max_output_bytes: usize,
    pub max_stderr_bytes: usize,
    pub timeout_ms: u64,
    pub max_runtime_ms: u64,
    pub artifact_max_bytes: u64,
    pub kill_on_limit: bool,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for GitRunOptions {
    fn default() -> Self {
        Self {
            output_mode: GitOutputMode::InlineOrArtifact,
            max_output_bytes: 512 * 1024,
            max_stderr_bytes: 128 * 1024,
            timeout_ms: 30_000,
            max_runtime_ms: 30_000,
            artifact_max_bytes: 256 * 1024 * 1024,
            kill_on_limit: false,
            limit: 200,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitPathValue {
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_bytes_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchMetadata {
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub oid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    pub kind: String,
    pub path: GitPathValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<GitPathValue>,
    pub index_status: String,
    pub worktree_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusData {
    pub branch: GitBranchMetadata,
    pub entries: Vec<GitStatusEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitLogEntry {
    pub commit: String,
    pub short_commit: String,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitLogData {
    pub entries: Vec<GitLogEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchEntry {
    pub name: String,
    pub object_id: String,
    pub current: bool,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchData {
    pub entries: Vec<GitBranchEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitData {
    pub phase: String,
    pub commit_hash: Option<String>,
    pub hooks_included: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum GitStructuredOutput {
    Status(GitStatusData),
    Log(GitLogData),
    Branches(GitBranchData),
    Commit(GitCommitData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub artifact_bytes: u64,
    pub artifact_ref: Option<String>,
    pub artifact_sha256: Option<String>,
    pub first_output_ms: Option<u64>,
    pub elapsed_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured: Option<GitStructuredOutput>,
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
