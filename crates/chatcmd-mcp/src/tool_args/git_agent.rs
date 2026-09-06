tool_args!(GitDiffArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(GitLogArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubagentApprovalGrantArgs {
    /// Distinct names from subagentPolicy.approvalGrant.allowedTools only. Git/process and
    /// agent_* lifecycle tools are ineligible. This is NOT the child's tool allowlist.
    /// Omit approvalGrant unless an approved parent safe-read grant can cover it.
    allowed_tools: Vec<String>,
    path_scopes: Vec<String>,
    max_calls: u64,
    max_files_scanned: u64,
    max_bytes_read: u64,
}
tool_args!(SubagentStartArgs {
    name: String,
    request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_effects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dependencies: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acceptance: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_context_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructions_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_grant: Option<SubagentApprovalGrantArgs>
});
tool_args!(SubagentWaitArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    /// Optional descendant ID to read a report page without waiting for other children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subagent_id: Option<String>,
    /// Zero-based Unicode character offset returned by report.continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report_offset: Option<u64>,
    /// Immutable final event ID from report.continuation; required for offsets above zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report_version: Option<String>
});
