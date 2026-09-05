use super::*;

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
    #[serde(default = "default_fs_mutation_max_open_files")]
    pub max_open_files: u32,
}

impl Default for FsMutationBudget {
    fn default() -> Self {
        Self {
            timeout_ms: default_fs_mutation_timeout_ms(),
            max_files: default_fs_mutation_max_files(),
            max_bytes_read: default_fs_mutation_max_bytes(),
            max_bytes_written: default_fs_mutation_max_bytes(),
            max_open_files: default_fs_mutation_max_open_files(),
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
const fn default_fs_mutation_max_open_files() -> u32 {
    32
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
    pub index_freshness: IndexFreshness,
    pub stale_entries_detected: u64,
}
