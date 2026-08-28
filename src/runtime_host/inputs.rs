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
input!(CwdInput { cwd: Option<PathBuf> });
input!(SkillInput { skill_id: String });
input!(ProcessInput { process_id: u32 });
input!(ArtifactInput {
    artifact_id: String
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
    overwrite: bool
});
input!(DeleteInput {
    path: PathBuf,
    #[serde(default)]
    recursive: bool
});
input!(GitShow {
    cwd: Option<PathBuf>,
    revision: String,
    #[serde(default)]
    path: Option<String>
});
input!(GitCommit {
    cwd: Option<PathBuf>,
    message: String,
    #[serde(default = "default_true")]
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
    #[serde(alias = "cwd")]
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
    #[serde(default, alias = "startSequence")]
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
    #[serde(default = "default_start_line")]
    pub(super) start_line: usize,
    #[serde(default)]
    pub(super) line_count: Option<usize>,
}

const fn default_characters() -> usize {
    200_000
}

const fn default_start_line() -> usize {
    1
}

input!(WriteTextInput {
    path: PathBuf,
    content: String,
    #[serde(default)]
    overwrite: bool
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
    base64: String,
    #[serde(default)]
    overwrite: bool
});

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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitLog {
    pub(super) cwd: Option<PathBuf>,
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
    use super::{
        CwdInput, GitCommit, GitDiff, ReadInput, ReplaceTextInput, ShellCreate, ShellRead,
        ShellSignalInput, ShellWrite,
    };

    #[test]
    fn git_cwd_can_be_omitted() {
        let status: CwdInput =
            serde_json::from_value(serde_json::json!({})).expect("git status input");
        assert!(status.cwd.is_none());

        let diff: GitDiff =
            serde_json::from_value(serde_json::json!({"stat": true})).expect("git diff input");
        assert!(diff.cwd.is_none());
        assert!(diff.stat);
    }

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

    #[test]
    fn shell_create_accepts_legacy_cwd_alias() {
        let input: ShellCreate = serde_json::from_value(serde_json::json!({
            "cwd": "."
        }))
        .expect("legacy shell create cwd");
        assert_eq!(
            input.working_directory.as_deref(),
            Some(std::path::Path::new("."))
        );
    }

    #[test]
    fn shell_write_accepts_legacy_input_alias() {
        let input: ShellWrite = serde_json::from_value(serde_json::json!({
            "sessionId": "session-1",
            "input": "echo ok"
        }))
        .expect("legacy shell write input");
        assert_eq!(input.text, "echo ok");
        assert!(input.append_new_line);
    }

    #[test]
    fn shell_read_accepts_legacy_start_sequence_alias() {
        let input: ShellRead = serde_json::from_value(serde_json::json!({
            "sessionId": "session-1",
            "startSequence": 7,
            "maxEvents": 50
        }))
        .expect("legacy shell read start sequence");
        assert_eq!(input.after_sequence, 7);
        assert_eq!(input.max_events, 50);
    }

    #[test]
    fn git_commit_stages_all_by_default() {
        let input: GitCommit = serde_json::from_value(serde_json::json!({
            "cwd": ".",
            "message": "test"
        }))
        .expect("git commit input");
        assert!(input.all);
    }

    #[test]
    fn shell_signal_accepts_sigint_alias() {
        let input: ShellSignalInput = serde_json::from_value(serde_json::json!({
            "sessionId": "session-1",
            "signal": "SIGINT"
        }))
        .expect("legacy SIGINT signal");
        assert!(matches!(input.signal, chatcmd_runtime::ShellSignal::CtrlC));
    }

    #[test]
    fn fs_read_text_accepts_line_range_fields() {
        let input: ReadInput = serde_json::from_value(serde_json::json!({
            "path": "test.txt",
            "startLine": 10,
            "lineCount": 25
        }))
        .expect("line range input");
        assert_eq!(input.start_line, 10);
        assert_eq!(input.line_count, Some(25));
    }

    #[test]
    fn fs_replace_text_defaults_to_one_expected_occurrence() {
        let input: ReplaceTextInput = serde_json::from_value(serde_json::json!({
            "path": "test.txt",
            "oldText": "before",
            "newText": "after"
        }))
        .expect("replace text input");
        assert_eq!(input.expected_occurrences, 1);
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
