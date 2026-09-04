use crate::{
    CommandOutput, CursorCodec, GitCommitData, GitOutputMode, GitRunOptions, GitStructuredOutput,
    RuntimeError, RuntimeResult, WorkspaceService,
    git_parser::{cursor_scope, parse_branches, parse_log, parse_status, structured_source},
    process_runner::BoundedProcessRunner,
};
use std::path::Path;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct GitService {
    workspace: WorkspaceService,
    default_options: GitRunOptions,
    runner: BoundedProcessRunner,
    cursor_codec: CursorCodec,
}

impl GitService {
    #[must_use]
    pub fn new(workspace: WorkspaceService, max_characters: usize) -> Self {
        let max_characters = max_characters.max(1024);
        let default_options = GitRunOptions {
            max_output_bytes: max_characters,
            max_stderr_bytes: max_characters.min(128 * 1024),
            ..GitRunOptions::default()
        };
        let artifact_directory = std::env::temp_dir().join("chatcmd-artifacts").join("git");
        Self {
            workspace,
            default_options,
            runner: BoundedProcessRunner::new(artifact_directory),
            cursor_codec: CursorCodec::ephemeral(),
        }
    }

    #[must_use]
    pub fn with_workspace(&self, workspace: WorkspaceService) -> Self {
        Self {
            workspace,
            default_options: self.default_options.clone(),
            runner: self.runner.clone(),
            cursor_codec: self.cursor_codec.clone(),
        }
    }

    pub async fn status(&self, cwd: &Path) -> RuntimeResult<CommandOutput> {
        self.status_with_options(cwd, &self.default_options, CancellationToken::new())
            .await
    }

    pub async fn status_with_options(
        &self,
        cwd: &Path,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let mut output = self
            .run(
                cwd,
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "--untracked-files=all",
                ],
                options,
                cancellation,
            )
            .await?;
        if output.exit_code == Some(0) && !output.cancelled && !output.timed_out {
            let source = structured_source(&output);
            let codec = self.cursor_codec.clone();
            let scope = cursor_scope(cwd, "status");
            let options = options.clone();
            let structured =
                tokio::task::spawn_blocking(move || parse_status(source, &options, &codec, &scope))
                    .await
                    .map_err(join_error)??;
            output.structured = Some(GitStructuredOutput::Status(structured));
        }
        Ok(output)
    }

    pub async fn diff(
        &self,
        cwd: &Path,
        staged: bool,
        stat: bool,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        self.diff_with_options(
            cwd,
            staged,
            stat,
            path,
            &self.default_options,
            CancellationToken::new(),
        )
        .await
    }

    pub async fn diff_with_options(
        &self,
        cwd: &Path,
        staged: bool,
        stat: bool,
        path: Option<&str>,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let args = git_diff_args(staged, stat, path);
        self.run_owned(cwd, &args, options, cancellation).await
    }

    pub async fn log(
        &self,
        cwd: &Path,
        count: usize,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        self.log_with_options(
            cwd,
            count,
            path,
            &self.default_options,
            CancellationToken::new(),
        )
        .await
    }

    pub async fn log_with_options(
        &self,
        cwd: &Path,
        count: usize,
        path: Option<&str>,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let mut args = vec![
            "log".to_owned(),
            "--no-color".to_owned(),
            "--format=%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e".to_owned(),
            format!("--max-count={}", count.clamp(1, 10_000)),
        ];
        if let Some(path) = path {
            args.extend(["--".to_owned(), path.to_owned()]);
        }
        let mut output = self.run_owned(cwd, &args, options, cancellation).await?;
        if output.exit_code == Some(0) && !output.cancelled && !output.timed_out {
            let source = structured_source(&output);
            let codec = self.cursor_codec.clone();
            let scope = cursor_scope(cwd, &format!("log:{count}:{}", path.unwrap_or_default()));
            let options = options.clone();
            let structured =
                tokio::task::spawn_blocking(move || parse_log(source, &options, &codec, &scope))
                    .await
                    .map_err(join_error)??;
            output.structured = Some(GitStructuredOutput::Log(structured));
        }
        Ok(output)
    }

    pub async fn branch(&self, cwd: &Path) -> RuntimeResult<CommandOutput> {
        self.branch_with_options(cwd, &self.default_options, CancellationToken::new())
            .await
    }

    pub async fn branch_with_options(
        &self,
        cwd: &Path,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let mut output = self
            .run(
                cwd,
                &[
                    "branch",
                    "--list",
                    "--all",
                    "--no-color",
                    "--format=%(refname)%09%(objectname)%09%(HEAD)%09%(upstream:short)",
                ],
                options,
                cancellation,
            )
            .await?;
        if output.exit_code == Some(0) && !output.cancelled && !output.timed_out {
            let source = structured_source(&output);
            let codec = self.cursor_codec.clone();
            let scope = cursor_scope(cwd, "branch");
            let options = options.clone();
            let structured = tokio::task::spawn_blocking(move || {
                parse_branches(source, &options, &codec, &scope)
            })
            .await
            .map_err(join_error)??;
            output.structured = Some(GitStructuredOutput::Branches(structured));
        }
        Ok(output)
    }

    pub async fn show(
        &self,
        cwd: &Path,
        revision: &str,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        self.show_with_options(
            cwd,
            revision,
            path,
            &self.default_options,
            CancellationToken::new(),
        )
        .await
    }

    pub async fn show_with_options(
        &self,
        cwd: &Path,
        revision: &str,
        path: Option<&str>,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        validate_revision(revision)?;
        let mut args = vec![
            "show".to_owned(),
            "--no-color".to_owned(),
            "--no-ext-diff".to_owned(),
            revision.to_owned(),
        ];
        if let Some(path) = path {
            args.extend(["--".to_owned(), path.to_owned()]);
        }
        self.run_owned(cwd, &args, options, cancellation).await
    }

    pub async fn commit(
        &self,
        cwd: &Path,
        message: &str,
        all: bool,
        paths: &[String],
    ) -> RuntimeResult<CommandOutput> {
        self.commit_with_options(
            cwd,
            message,
            all,
            paths,
            &self.default_options,
            CancellationToken::new(),
        )
        .await
    }

    pub async fn commit_with_options(
        &self,
        cwd: &Path,
        message: &str,
        all: bool,
        paths: &[String],
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        if message.trim().is_empty() {
            return Err(RuntimeError::new(
                "invalid_commit_message",
                "commit message cannot be empty",
            ));
        }
        if !paths.is_empty() {
            let mut args = vec!["add".to_owned(), "--".to_owned()];
            args.extend(paths.iter().cloned());
            let mut staged = self
                .run_owned(cwd, &args, options, cancellation.clone())
                .await?;
            if staged.exit_code != Some(0) || staged.timed_out || staged.cancelled {
                staged.structured = Some(GitStructuredOutput::Commit(GitCommitData {
                    phase: "stage".to_owned(),
                    commit_hash: None,
                    hooks_included: false,
                }));
                return Ok(staged);
            }
        } else if all {
            let mut staged = self
                .run(cwd, &["add", "--all"], options, cancellation.clone())
                .await?;
            if staged.exit_code != Some(0) || staged.timed_out || staged.cancelled {
                staged.structured = Some(GitStructuredOutput::Commit(GitCommitData {
                    phase: "stage".to_owned(),
                    commit_hash: None,
                    hooks_included: false,
                }));
                return Ok(staged);
            }
        }
        let mut committed = self
            .run(
                cwd,
                &["commit", "--message", message],
                options,
                cancellation.clone(),
            )
            .await?;
        let commit_hash =
            if committed.exit_code == Some(0) && !committed.timed_out && !committed.cancelled {
                self.commit_hash(cwd, options, cancellation).await?
            } else {
                None
            };
        committed.structured = Some(GitStructuredOutput::Commit(GitCommitData {
            phase: "commitHooksIncluded".to_owned(),
            commit_hash,
            hooks_included: true,
        }));
        Ok(committed)
    }

    async fn commit_hash(
        &self,
        cwd: &Path,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<Option<String>> {
        let mut bounded = options.clone();
        bounded.output_mode = GitOutputMode::Inline;
        bounded.max_output_bytes = bounded.max_output_bytes.clamp(64, 4096);
        bounded.max_stderr_bytes = bounded.max_stderr_bytes.clamp(64, 4096);
        bounded.cursor = None;
        bounded.limit = 1;
        let output = self
            .run(cwd, &["rev-parse", "HEAD"], &bounded, cancellation)
            .await?;
        if output.exit_code == Some(0) && !output.timed_out && !output.cancelled {
            let hash = output.stdout.trim();
            if hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Ok(Some(hash.to_owned()));
            }
        }
        Ok(None)
    }

    async fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let args = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        self.run_owned(cwd, &args, options, cancellation).await
    }

    async fn run_owned(
        &self,
        cwd: &Path,
        args: &[String],
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        validate_options(options)?;
        let cwd = self.workspace.stat(cwd).await?.path;
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("NO_COLOR", "1");
        self.runner.run(command, options, cancellation).await
    }
}

fn git_diff_args(staged: bool, stat: bool, path: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--no-ext-diff".to_owned(),
        "--no-color".to_owned(),
    ];
    if staged {
        args.push("--cached".to_owned());
    }
    if stat {
        args.push("--stat".to_owned());
    }
    if let Some(path) = path {
        args.extend(["--".to_owned(), path.to_owned()]);
    }
    args
}

fn validate_revision(value: &str) -> RuntimeResult<()> {
    if value.is_empty() || value.starts_with('-') || value.contains(['\0', '\n', '\r']) {
        Err(RuntimeError::new(
            "invalid_revision",
            "git revision is invalid",
        ))
    } else {
        Ok(())
    }
}

fn validate_options(options: &GitRunOptions) -> RuntimeResult<()> {
    if options.max_output_bytes == 0
        || options.max_stderr_bytes == 0
        || options.timeout_ms == 0
        || options.max_runtime_ms == 0
        || options.artifact_max_bytes == 0
        || options.limit == 0
    {
        return Err(RuntimeError::new(
            "invalid_git_limits",
            "git output, runtime and item limits must be greater than zero",
        ));
    }
    Ok(())
}

fn join_error(error: tokio::task::JoinError) -> RuntimeError {
    RuntimeError::new("git_parse_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_diff_args_support_stat_and_path_separator() {
        assert_eq!(
            git_diff_args(true, true, Some("--output=/tmp/injected")),
            vec![
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--cached",
                "--stat",
                "--",
                "--output=/tmp/injected"
            ]
        );
    }

    #[test]
    fn revision_rejects_option_and_control_character_injection() {
        assert!(validate_revision("--help").is_err());
        assert!(validate_revision("HEAD\n--help").is_err());
        assert!(validate_revision("HEAD").is_ok());
    }
}
