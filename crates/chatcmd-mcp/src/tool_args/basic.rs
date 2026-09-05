tool_args!(NoArgs {});
tool_args!(DeviceGetArgs { device_id: String });
tool_args!(SessionArgs { session_id: String });
tool_args!(PathArgs { path: String });
tool_args!(ProjectContextArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_paths: Option<Vec<String>>,
    /// Explicit policy; CLAUDE.md is excluded unless loadClaudeMd is true.
    #[serde(default)]
    policy: chatcmd_runtime::ProjectContextPolicy,
    /// Bounded continuation for one previously returned rule record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<chatcmd_runtime::ProjectContextRange>
});
tool_args!(StatArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version_strength: Option<chatcmd_runtime::VersionStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash_algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsBatchStatBudget>
});
tool_args!(BatchStatArgs {
    paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version_strength: Option<chatcmd_runtime::VersionStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsStatBudget>
});
tool_args!(CwdArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(SkillArgs {
    #[serde(alias = "id")]
    skill_id: String
});
tool_args!(ProcessArgs { process_id: u32 });
tool_args!(ArtifactArgs {
    artifact_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<usize>
});
tool_args!(ArtifactCreateArgs {
    content_ref: String,
    relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_type: Option<String>
});
tool_args!(ExecutionModeArgs { mode: String });
tool_args!(ShellResizeArgs {
    session_id: String,
    columns: u16,
    rows: u16
});
tool_args!(TransferArgs {
    source: String,
    destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict_policy: Option<chatcmd_runtime::FsConflictPolicy>,
    #[serde(default = "default_true")]
    atomic_publish: bool,
    #[serde(default)]
    verify: chatcmd_runtime::FsVerifyMode,
    #[serde(default = "default_true")]
    preserve_metadata: bool,
    #[serde(default)]
    follow_symlinks: bool,
    #[serde(default)]
    dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_source_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_destination_version: Option<String>,
    #[serde(default)]
    budget: chatcmd_runtime::FsMutationBudget
});
tool_args!(DeleteArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
    #[serde(default)]
    mode: chatcmd_runtime::FsDeleteMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    budget: chatcmd_runtime::FsMutationBudget
});
tool_args!(QuarantineRestoreArgs {
    quarantine_path: String,
    destination: String,
    #[serde(default)]
    replace: bool
});
tool_args!(QuarantineGcArgs {
    path: String,
    #[serde(default = "default_quarantine_retention_seconds")]
    retention_seconds: u64,
    #[serde(default = "default_quarantine_max_total_bytes")]
    max_total_bytes: u64,
    #[serde(default = "default_quarantine_max_items")]
    max_items: u64,
    #[serde(default)]
    dry_run: bool
});
tool_args!(GitShowArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(GitCommitArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    message: String,
    /// Commit every already-staged change. Fails closed if the worktree also has
    /// unstaged or untracked changes; mutually exclusive with paths.
    #[serde(default, skip_serializing_if = "is_false")]
    all: bool,
    /// Explicit path scope. Must be non-empty when all is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
    /// Return a side-effect-free preview that can be supplied as expectedPreview.
    #[serde(default, skip_serializing_if = "is_false")]
    preview_only: bool,
    /// Rechecked immediately before commit; stale repository state is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_preview: Option<chatcmd_runtime::GitCommitPreview>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});

fn is_false(value: &bool) -> bool {
    !value
}
tool_args!(ProcessKillArgs {
    process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entire_tree: Option<bool>
});
tool_args!(UserMessageArgs { content: String });
tool_args!(ProgressArgs {
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>
});
tool_args!(PlanQuestionArgs {
    question: String,
    options: [String; 2],
    #[serde(default, skip_serializing_if = "is_clarification")]
    question_kind: PlanQuestionKindArgs
});

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
enum PlanQuestionKindArgs {
    #[default]
    Clarification,
    ExecutionConsent,
}

fn is_clarification(value: &PlanQuestionKindArgs) -> bool {
    matches!(value, PlanQuestionKindArgs::Clarification)
}
tool_args!(CompleteArgs {
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_outcome: Option<WorkOutcomeArgs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_intent: Option<VerificationIntentArgs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    criteria: Vec<CompletionCriterionArgs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    limitations: Vec<String>
});

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
enum WorkOutcomeArgs {
    Completed,
    Partial,
    Blocked,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
enum VerificationIntentArgs {
    NotRun,
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompletionCriterionArgs {
    criterion: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence_refs: Vec<String>,
}
tool_args!(ShellCreateArgs {
    #[serde(default, alias = "cwd", alias = "initialWorkingDirectory", skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rows: Option<u16>
});
tool_args!(CommandRunArgs {
    executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_stdout_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_stderr_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_artifact_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kill_on_output_limit: Option<bool>
});
tool_args!(ShellWriteArgs {
    session_id: String,
    #[serde(alias = "input")]
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    append_new_line: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_kind: Option<ShellInputKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sensitive: Option<bool>
});
tool_args!(ShellWaitArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>
});
tool_args!(ShellReadArgs {
    session_id: String,
    #[serde(default, alias = "startSequence", alias = "fromSequence", skip_serializing_if = "Option::is_none")]
    after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_events: Option<usize>
});
tool_args!(ShellSignalArgs {
    session_id: String,
    signal: String
});
tool_args!(ShellCloseArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    force: Option<bool>
});
tool_args!(ListArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>
});
