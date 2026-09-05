#[cfg(test)]
mod tests {
    use super::{
        GitCommit, GitCwdInput, GitDiff, ReadInput, ReplaceTextInput, ShellCreate, ShellRead,
        ShellSignalInput, ShellWrite,
    };
    use crate::runtime_host::plan_prompt::PlanQuestionKind;

    #[test]
    fn git_cwd_can_be_omitted() {
        let status: GitCwdInput =
            serde_json::from_value(serde_json::json!({})).expect("git status input");
        assert!(status.cwd.is_none());

        let diff: GitDiff =
            serde_json::from_value(serde_json::json!({"stat": true})).expect("git diff input");
        assert!(diff.cwd.is_none());
        assert!(diff.stat);
    }

    #[test]
    fn git_cwd_accepts_legacy_path_alias() {
        let status: GitCwdInput = serde_json::from_value(serde_json::json!({
            "path": "D:/DEV/CmdGPT/ChatCmdClient"
        }))
        .expect("legacy git cwd path");
        assert_eq!(
            status.cwd.as_deref(),
            Some(std::path::Path::new("D:/DEV/CmdGPT/ChatCmdClient"))
        );
    }

    #[test]
    fn git_diff_accepts_stat_flag() {
        let input: GitDiff = serde_json::from_value(serde_json::json!({
            "cwd": ".",
            "stat": true,
            "maxOutputBytes": 4096,
            "timeoutMs": 1250,
            "killOnLimit": true
        }))
        .expect("git diff stat input");
        assert!(input.stat);
        assert!(!input.staged);
        assert!(input.path.is_none());
        assert_eq!(input.options.max_output_bytes, 4096);
        assert_eq!(input.options.timeout_ms, 1250);
        assert!(input.options.kill_on_limit);
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
    fn shell_create_accepts_initial_working_directory_alias() {
        let input: ShellCreate = serde_json::from_value(serde_json::json!({
            "initialWorkingDirectory": "D:/DEV/CmdGPT/ChatCmdClient/web"
        }))
        .expect("legacy shell create initial working directory");
        assert_eq!(
            input.working_directory.as_deref(),
            Some(std::path::Path::new("D:/DEV/CmdGPT/ChatCmdClient/web"))
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
        assert_eq!(
            input.input_kind,
            chatcmd_runtime::ShellInputKind::Interactive
        );
        assert!(!input.sensitive);
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
    fn shell_read_accepts_from_sequence_alias() {
        let input: ShellRead = serde_json::from_value(serde_json::json!({
            "sessionId": "session-1",
            "fromSequence": 11
        }))
        .expect("legacy shell read from sequence");
        assert_eq!(input.after_sequence, 11);
    }

    #[test]
    fn git_commit_requires_explicit_all_opt_in() {
        let input: GitCommit = serde_json::from_value(serde_json::json!({
            "cwd": ".",
            "message": "test"
        }))
        .expect("git commit input");
        assert!(!input.all);
    }

    #[test]
    fn plan_question_kind_defaults_to_clarification_and_accepts_consent() {
        let legacy: super::PlanQuestionInput = serde_json::from_value(serde_json::json!({
            "question": "Choose", "options": ["A", "B"]
        }))
        .expect("legacy question");
        assert_eq!(legacy.question_kind, PlanQuestionKind::Clarification);
        let consent: super::PlanQuestionInput = serde_json::from_value(serde_json::json!({
            "question": "Execute", "options": ["Approve", "Deny"],
            "questionKind": "executionConsent"
        }))
        .expect("execution consent");
        assert_eq!(consent.question_kind, PlanQuestionKind::ExecutionConsent);
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
