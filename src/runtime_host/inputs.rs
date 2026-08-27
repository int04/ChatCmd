use std::{collections::BTreeMap, path::PathBuf};

use chatcmd_runtime::ShellSignal;
use serde::Deserialize;

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
input!(CwdInput { cwd: PathBuf });
input!(TaskInput { task_id: String });
input!(SkillInput { skill_id: String });
input!(ProcessInput { process_id: u32 });
input!(ArtifactInput {
    task_id: String,
    artifact_id: String
});
input!(ExecutionModeInput {
    task_id: String,
    mode: String
});
input!(ShellResize {
    session_id: String,
    columns: u16,
    rows: u16
});
input!(TransferInput {
    source: PathBuf,
    destination: PathBuf,
    #[serde(default)]
    overwrite: bool
});
input!(DeleteInput {
    path: PathBuf,
    #[serde(default)]
    recursive: bool
});
input!(GitShow {
    cwd: PathBuf,
    revision: String,
    #[serde(default)]
    path: Option<String>
});
input!(GitCommit {
    cwd: PathBuf,
    message: String,
    #[serde(default)]
    all: bool
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
pub(super) struct CompleteInput {
    pub(super) content: String,
    #[serde(default)]
    pub(super) suggested_title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShellCreate {
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
    pub(super) text: String,
    #[serde(default = "default_true")]
    pub(super) append_new_line: bool,
}

const fn default_true() -> bool {
    true
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
    #[serde(default)]
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
pub(super) struct SearchInput {
    pub(super) path: PathBuf,
    pub(super) query: String,
    #[serde(default)]
    pub(super) case_sensitive: bool,
    #[serde(default = "default_limit")]
    pub(super) max_results: usize,
    #[serde(default = "default_file_bytes")]
    pub(super) max_file_bytes: u64,
}

const fn default_file_bytes() -> u64 {
    1_048_576
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FindInput {
    pub(super) path: PathBuf,
    pub(super) pattern: String,
    #[serde(default = "default_limit")]
    pub(super) max_results: usize,
    #[serde(default = "default_depth")]
    pub(super) max_depth: usize,
}

const fn default_depth() -> usize {
    32
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReadInput {
    pub(super) path: PathBuf,
    #[serde(default = "default_characters")]
    pub(super) max_characters: usize,
}

const fn default_characters() -> usize {
    200_000
}

input!(WriteTextInput {
    path: PathBuf,
    content: String,
    #[serde(default)]
    overwrite: bool
});
input!(WriteRawInput {
    path: PathBuf,
    base64: String,
    #[serde(default)]
    overwrite: bool
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitDiff {
    pub(super) cwd: PathBuf,
    #[serde(default)]
    pub(super) staged: bool,
    #[serde(default)]
    pub(super) stat: bool,
    #[serde(default)]
    pub(super) path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitLog {
    pub(super) cwd: PathBuf,
    #[serde(default = "default_git_count")]
    pub(super) count: usize,
    #[serde(default)]
    pub(super) path: Option<String>,
}

const fn default_git_count() -> usize {
    20
}

#[cfg(test)]
mod tests {
    use super::GitDiff;

    #[test]
    fn git_diff_accepts_stat_flag() {
        let input: GitDiff = serde_json::from_value(serde_json::json!({
            "cwd": ".",
            "stat": true
        }))
        .expect("git diff stat input");
        assert!(input.stat);
        assert!(!input.staged);
        assert!(input.path.is_none());
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubagentStartInput {
    pub(super) name: String,
    pub(super) request: String,
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
