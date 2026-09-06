use super::*;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryIndexEntrySnapshot {
    pub relative_path_bytes: Vec<u8>,
    pub display_path: String,
    pub entry_type: String,
    pub size_bytes: u64,
    pub modified_at_ns: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryIndexSnapshot {
    pub root: PathBuf,
    pub generation: u64,
    pub freshness: IndexFreshness,
    pub indexed_bytes: u64,
    pub schema_version: u32,
    pub entries: Vec<RepositoryIndexEntrySnapshot>,
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
