use std::{collections::BTreeMap, path::PathBuf};

use chatcmd_runtime::{
    BlobToolBudget, FindEntryType, FindPatternMode, FsConflictPolicy, FsDeleteMode, FsFindBudget,
    FsListBudget, FsListMetadata, FsListSort, FsMutationBudget, FsStatBudget, FsVerifyMode,
    ShellSignal, VersionStrength,
};
use serde::{Deserialize, Serialize};

use super::plan_prompt::PlanQuestionKind;

macro_rules! input {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub(super) struct $name { $( $(#[$meta])* pub(super) $field: $ty, )* }
    };
}

input!(DeviceGet { device_id: String });
input!(SessionInput { session_id: String });
input!(PathInput { path: PathBuf });
input!(ProjectContextInput {
    #[serde(default)]
    target_paths: Vec<PathBuf>,
    #[serde(default)]
    policy: chatcmd_runtime::ProjectContextPolicy,
    #[serde(default)]
    range: Option<chatcmd_runtime::ProjectContextRange>
});
input!(StatInput {
    path: PathBuf,
    #[serde(default)]
    version_strength: VersionStrength,
    #[serde(default)]
    hash_algorithm: Option<String>,
    #[serde(default)]
    budget: FsStatBudget
});
input!(SkillInput {
    #[serde(alias = "id")]
    skill_id: String
});
input!(ProcessInput { process_id: u32 });
input!(ArtifactInput {
    artifact_id: String,
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_artifact_read_max")]
    max_bytes: usize
});
input!(ArtifactCreateInput {
    content_ref: String,
    relative_path: String,
    #[serde(default)]
    media_type: Option<String>
});
input!(ExecutionModeInput { mode: String });
input!(ShellResize {
    session_id: String,
    columns: u16,
    rows: u16
});
input!(TransferInput {
    source: PathBuf,
    destination: PathBuf,
    #[serde(default)]
    overwrite: Option<bool>,
    #[serde(default)]
    conflict_policy: Option<FsConflictPolicy>,
    #[serde(default = "default_true")]
    atomic_publish: bool,
    #[serde(default)]
    verify: FsVerifyMode,
    #[serde(default = "default_true")]
    preserve_metadata: bool,
    #[serde(default)]
    follow_symlinks: bool,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    expected_source_version: Option<String>,
    #[serde(default)]
    expected_destination_version: Option<String>,
    #[serde(default)]
    budget: FsMutationBudget
});
input!(GitCwdInput {
    #[serde(alias = "path")]
    cwd: Option<PathBuf>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
input!(DeleteInput {
    path: PathBuf,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    mode: FsDeleteMode,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    budget: FsMutationBudget
});
input!(QuarantineRestoreInput {
    quarantine_path: PathBuf,
    destination: PathBuf,
    #[serde(default)]
    replace: bool
});
input!(QuarantineGcInput {
    path: PathBuf,
    #[serde(default = "default_quarantine_retention_seconds")]
    retention_seconds: u64,
    #[serde(default = "default_quarantine_max_total_bytes")]
    max_total_bytes: u64,
    #[serde(default = "default_quarantine_max_items")]
    max_items: u64,
    #[serde(default)]
    dry_run: bool
});
input!(GitShow {
    cwd: Option<PathBuf>,
    revision: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
input!(GitCommit {
    cwd: Option<PathBuf>,
    message: String,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    preview_only: bool,
    #[serde(default)]
    expected_preview: Option<chatcmd_runtime::GitCommitPreview>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
input!(ProcessKill {
    process_id: u32,
    #[serde(default)]
    entire_tree: bool
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UserMessageInput {
    pub(super) content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProgressInput {
    pub(super) message: String,
    #[serde(default)]
    pub(super) suggested_title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PlanQuestionInput {
    pub(super) question: String,
    pub(super) options: [String; 2],
    #[serde(default)]
    pub(super) question_kind: PlanQuestionKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellCreate {
    #[serde(alias = "cwd", alias = "initialWorkingDirectory")]
    pub(super) working_directory: Option<PathBuf>,
    pub(super) executable: Option<PathBuf>,
    #[serde(default)]
    pub(super) arguments: Vec<String>,
    #[serde(default)]
    pub(super) environment: BTreeMap<String, String>,
    pub(super) columns: Option<u16>,
    pub(super) rows: Option<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellWrite {
    pub(super) session_id: String,
    #[serde(alias = "input")]
    pub(super) text: String,
    #[serde(default = "default_true")]
    pub(super) append_new_line: bool,
    #[serde(default)]
    pub(super) input_kind: chatcmd_runtime::ShellInputKind,
    #[serde(default)]
    pub(super) sensitive: bool,
}

const fn default_true() -> bool {
    true
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellWait {
    pub(super) session_id: String,
    #[serde(default = "default_timeout")]
    pub(super) timeout_ms: u64,
}

const fn default_timeout() -> u64 {
    30_000
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellRead {
    pub(super) session_id: String,
    #[serde(default, alias = "startSequence", alias = "fromSequence")]
    pub(super) after_sequence: u64,
    #[serde(default = "default_limit")]
    pub(super) max_events: usize,
}

const fn default_limit() -> usize {
    200
}

input!(ShellSignalInput {
    session_id: String,
    signal: ShellSignal
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellClose {
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) force: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListInput {
    pub(super) path: PathBuf,
    #[serde(default)]
    pub(super) offset: usize,
    #[serde(default = "default_limit")]
    pub(super) limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListV2Input {
    pub(super) path: PathBuf,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub(super) limit: usize,
    #[serde(default)]
    pub(super) sort: FsListSort,
    #[serde(default)]
    pub(super) metadata: Vec<FsListMetadata>,
    #[serde(default = "default_true")]
    pub(super) include_hidden: bool,
    #[serde(default)]
    pub(super) budget: FsListBudget,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SearchInput {
    pub(super) path: PathBuf,
    pub(super) query: String,
    #[serde(default)]
    pub(super) mode: Option<chatcmd_runtime::SearchMode>,
    #[serde(default)]
    pub(super) case_sensitive: bool,
    #[serde(default)]
    pub(super) word_boundary: bool,
    #[serde(default)]
    pub(super) include: Vec<String>,
    #[serde(default)]
    pub(super) exclude: Vec<String>,
    #[serde(default)]
    pub(super) include_ignored: bool,
    #[serde(default)]
    pub(super) context_before: usize,
    #[serde(default)]
    pub(super) context_after: usize,
    #[serde(default)]
    pub(super) max_matches_per_file: Option<usize>,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) max_results: Option<usize>,
    #[serde(default)]
    pub(super) max_file_bytes: Option<u64>,
    #[serde(default)]
    pub(super) max_snippet_bytes: Option<usize>,
    #[serde(default)]
    pub(super) budget: Option<chatcmd_runtime::FsSearchBudget>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FindInput {
    pub(super) path: PathBuf,
    pub(super) pattern: String,
    #[serde(default)]
    pub(super) pattern_mode: Option<FindPatternMode>,
    #[serde(default)]
    pub(super) case_sensitive: bool,
    #[serde(default)]
    pub(super) entry_types: Vec<FindEntryType>,
    #[serde(default = "default_find_depth")]
    pub(super) max_depth: usize,
    #[serde(default)]
    pub(super) include_ignored: bool,
    #[serde(default)]
    pub(super) include_hidden: bool,
    #[serde(default)]
    pub(super) exclude: Vec<String>,
    #[serde(default)]
    pub(super) extensions: Vec<String>,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) max_results: Option<usize>,
    #[serde(default)]
    pub(super) budget: FsFindBudget,
}

const fn default_find_depth() -> usize {
    64
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReadInput {
    pub(super) path: PathBuf,
    #[serde(default = "default_characters")]
    pub(super) max_characters: usize,
    #[serde(default = "default_start_line")]
    pub(super) start_line: usize,
    #[serde(default)]
    pub(super) line_count: Option<usize>,
}

const fn default_characters() -> usize {
    200_000
}

const fn default_artifact_read_max() -> usize {
    200_000
}

const fn default_start_line() -> usize {
    1
}

input!(WriteTextInput {
    path: PathBuf,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_ref: Option<String>,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    metadata_policy: chatcmd_runtime::MetadataPolicy,
    #[serde(default)]
    durability: chatcmd_runtime::DurabilityMode,
    #[serde(default)]
    require_atomic: bool
});
input!(ReplaceTextInput {
    path: PathBuf,
    old_text: String,
    new_text: String,
    #[serde(default = "default_expected_occurrences")]
    expected_occurrences: usize
});
input!(WriteRawInput {
    path: PathBuf,
    #[serde(default)]
    base64: Option<String>,
    #[serde(default)]
    content_ref: Option<String>,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    metadata_policy: chatcmd_runtime::MetadataPolicy,
    #[serde(default)]
    durability: chatcmd_runtime::DurabilityMode,
    #[serde(default)]
    require_atomic: bool
});

input!(BlobStatusInput {
    upload_id: String,
    #[serde(default)]
    budget: BlobToolBudget
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ApplyEditsInput {
    pub(super) path: PathBuf,
    pub(super) expected_version: String,
    pub(super) coordinate_system: chatcmd_runtime::EditCoordinateSystem,
    #[serde(default)]
    pub(super) column_encoding: Option<chatcmd_runtime::EditColumnEncoding>,
    #[serde(default)]
    pub(super) edits: Option<Vec<chatcmd_runtime::TextEdit>>,
    #[serde(default)]
    pub(super) content_ref: Option<String>,
    #[serde(default)]
    pub(super) dry_run: bool,
    #[serde(default = "default_true")]
    pub(super) preserve_line_endings: bool,
    #[serde(default = "default_true")]
    pub(super) preserve_bom: bool,
    #[serde(default)]
    pub(super) budget: chatcmd_runtime::ApplyEditsBudget,
}

const fn default_expected_occurrences() -> usize {
    1
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitDiff {
    pub(super) cwd: Option<PathBuf>,
    #[serde(default)]
    pub(super) staged: bool,
    #[serde(default)]
    pub(super) stat: bool,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(flatten, default)]
    pub(super) options: chatcmd_runtime::GitRunOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitLog {
    pub(super) cwd: Option<PathBuf>,
    #[serde(default = "default_git_count")]
    pub(super) count: usize,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(flatten, default)]
    pub(super) options: chatcmd_runtime::GitRunOptions,
}

const fn default_git_count() -> usize {
    20
}

include!("inputs_tests.rs");
include!("inputs_subagent.rs");
