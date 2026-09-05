use super::*;

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
    pub index_used: bool,
    pub index_generation: Option<u64>,
    pub index_freshness: IndexFreshness,
    pub stale_entries_detected: u64,
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
    pub index_used: bool,
    pub index_generation: Option<u64>,
    pub index_freshness: IndexFreshness,
    pub stale_entries_detected: u64,
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
