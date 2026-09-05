#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubagentApprovalGrantInput {
    pub(super) allowed_tools: Vec<String>,
    pub(super) path_scopes: Vec<String>,
    pub(super) max_calls: u64,
    pub(super) max_files_scanned: u64,
    pub(super) max_bytes_read: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubagentStartInput {
    pub(super) name: String,
    pub(super) request: String,
    #[serde(default)]
    pub(super) allowed_files: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_effects: Option<Vec<String>>,
    #[serde(default)]
    pub(super) dependencies: Option<Vec<String>>,
    #[serde(default)]
    pub(super) acceptance: Option<Vec<String>>,
    #[serde(default)]
    pub(super) project_context_ref: Option<String>,
    #[serde(default)]
    pub(super) instructions_version: Option<String>,
    #[serde(default)]
    pub(super) approval_grant: Option<SubagentApprovalGrantInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubagentWaitInput {
    #[serde(default = "default_subagent_wait_ms")]
    pub(super) timeout_ms: u64,
}

const fn default_subagent_wait_ms() -> u64 {
    20_000
}
