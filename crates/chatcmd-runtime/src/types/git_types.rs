use super::*;

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
