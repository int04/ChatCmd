use crate::{
    CommandOutput, OperationContext, PolicyContext, PolicyEngine, ProcessInfo, RuntimeError,
    RuntimeResult, WorkspaceService,
};
use std::{path::Path, process::Stdio};
use tokio::process::Command;

#[derive(Clone)]
pub struct GitService {
    workspace: WorkspaceService,
    max_characters: usize,
}

impl GitService {
    #[must_use]
    pub fn new(workspace: WorkspaceService, max_characters: usize) -> Self {
        Self {
            workspace,
            max_characters: max_characters.max(1024),
        }
    }

    #[must_use]
    pub fn with_workspace(&self, workspace: WorkspaceService) -> Self {
        Self {
            workspace,
            max_characters: self.max_characters,
        }
    }

    pub async fn status(&self, cwd: &Path) -> RuntimeResult<CommandOutput> {
        self.run(cwd, &["status", "--short", "--branch"]).await
    }
    pub async fn diff(
        &self,
        cwd: &Path,
        staged: bool,
        stat: bool,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        let args = git_diff_args(staged, stat, path);
        self.run(cwd, &args).await
    }
    pub async fn log(
        &self,
        cwd: &Path,
        count: usize,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        let count_arg = format!("--max-count={}", count.clamp(1, 200));
        let mut args = vec!["log", "--oneline", "--decorate", count_arg.as_str()];
        if let Some(path) = path {
            args.extend(["--", path]);
        }
        self.run(cwd, &args).await
    }
    pub async fn branch(&self, cwd: &Path) -> RuntimeResult<CommandOutput> {
        self.run(cwd, &["branch", "--list", "--all", "--no-color"])
            .await
    }
    pub async fn show(
        &self,
        cwd: &Path,
        revision: &str,
        path: Option<&str>,
    ) -> RuntimeResult<CommandOutput> {
        validate_revision(revision)?;
        let specification =
            path.map_or_else(|| revision.to_owned(), |path| format!("{revision}:{path}"));
        self.run(cwd, &["show", "--no-color", &specification]).await
    }
    pub async fn commit(
        &self,
        cwd: &Path,
        message: &str,
        all: bool,
        paths: &[String],
    ) -> RuntimeResult<CommandOutput> {
        if message.trim().is_empty() {
            return Err(RuntimeError::new(
                "invalid_commit_message",
                "commit message cannot be empty",
            ));
        }
        if !paths.is_empty() {
            let cwd = self.workspace.stat(cwd).await?.path;
            let mut command = Command::new("git");
            command.arg("add").arg("--").args(paths).current_dir(&cwd);
            let output = command.output().await.map_err(command_error)?;
            let staged = bound_output(output, self.max_characters);
            if staged.exit_code != Some(0) {
                return Ok(staged);
            }
        } else if all {
            let staged = self.run(cwd, &["add", "--all"]).await?;
            if staged.exit_code != Some(0) {
                return Ok(staged);
            }
        }
        self.run(cwd, &["commit", "--message", message]).await
    }
    async fn run(&self, cwd: &Path, args: &[&str]) -> RuntimeResult<CommandOutput> {
        let cwd = self.workspace.stat(cwd).await?.path;
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = command.output().await.map_err(command_error)?;
        Ok(bound_output(output, self.max_characters))
    }
}

fn git_diff_args(staged: bool, stat: bool, path: Option<&str>) -> Vec<&str> {
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    if stat {
        args.push("--stat");
    }
    if let Some(path) = path {
        args.extend(["--", path]);
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

#[derive(Clone)]
pub struct ProcessService {
    policy: PolicyEngine,
}

impl ProcessService {
    #[must_use]
    pub fn new(policy: PolicyEngine) -> Self {
        Self { policy }
    }
    pub async fn list(&self) -> RuntimeResult<Vec<ProcessInfo>> {
        let output = if cfg!(windows) {
            Command::new("tasklist.exe")
                .args(["/FO", "CSV", "/NH"])
                .output()
                .await
        } else {
            Command::new("ps")
                .args(["-eo", "pid=,comm=,args="])
                .output()
                .await
        }
        .map_err(command_error)?;
        if !output.status.success() {
            return Err(RuntimeError::new(
                "process_list_failed",
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut values = Vec::new();
        for line in text.lines().take(10_000) {
            if cfg!(windows) {
                let fields: Vec<_> = line.trim_matches('"').split("\",\"").collect();
                if fields.len() >= 2
                    && let Ok(pid) = fields[1].replace(',', "").parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: fields[0].into(),
                        details: line.into(),
                    });
                }
            } else {
                let mut parts = line.trim().splitn(3, char::is_whitespace);
                if let (Some(pid), Some(name)) = (parts.next(), parts.next())
                    && let Ok(pid) = pid.parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: name.into(),
                        details: parts.next().unwrap_or_default().into(),
                    });
                }
            }
        }
        Ok(values)
    }
    pub async fn inspect(&self, process_id: u32) -> RuntimeResult<ProcessInfo> {
        self.list()
            .await?
            .into_iter()
            .find(|process| process.process_id == process_id)
            .ok_or_else(|| RuntimeError::new("process_not_found", "process was not found"))
    }
    pub async fn kill(
        &self,
        context: &OperationContext,
        process_id: u32,
        entire_tree: bool,
    ) -> RuntimeResult<()> {
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "process_kill".into(),
                root: None,
                destructive: true,
            })
            .await?;
        let output = if cfg!(windows) {
            let mut command = Command::new("taskkill.exe");
            command.args(["/PID", &process_id.to_string(), "/F"]);
            if entire_tree {
                command.arg("/T");
            }
            command.output().await
        } else {
            Command::new("kill")
                .args(["-TERM", &process_id.to_string()])
                .output()
                .await
        }
        .map_err(command_error)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "process_kill_failed",
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ))
        }
    }
}

fn bound_output(output: std::process::Output, limit: usize) -> CommandOutput {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_truncated = stdout.chars().count() > limit;
    let stderr_truncated = stderr.chars().count() > limit;
    CommandOutput {
        exit_code: output.status.code(),
        stdout: stdout.chars().take(limit).collect(),
        stderr: stderr.chars().take(limit).collect(),
        truncated: stdout_truncated || stderr_truncated,
    }
}
fn command_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("process_start_failed", error.to_string())
}
#[cfg(test)]
mod tests {
    use super::git_diff_args;

    #[test]
    fn git_diff_args_support_stat() {
        assert_eq!(git_diff_args(false, true, None), vec!["diff", "--stat"]);
        assert_eq!(
            git_diff_args(true, true, Some("src/main.rs")),
            vec!["diff", "--cached", "--stat", "--", "src/main.rs"]
        );
    }
}
