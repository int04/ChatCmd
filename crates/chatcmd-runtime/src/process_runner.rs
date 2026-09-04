use crate::{CommandOutput, GitOutputMode, GitRunOptions, RuntimeError, RuntimeResult};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Instant};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

const READ_CHUNK_BYTES: usize = 32 * 1024;
const HARD_PROCESS_RUNTIME_MS: u64 = 10 * 60 * 1_000;
const HARD_STDOUT_PREVIEW_BYTES: usize = 4 * 1024 * 1024;
const HARD_STDERR_PREVIEW_BYTES: usize = 1024 * 1024;
const HARD_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct BoundedProcessRunner {
    artifact_directory: Arc<PathBuf>,
}

impl BoundedProcessRunner {
    pub(crate) fn new(artifact_directory: PathBuf) -> Self {
        Self {
            artifact_directory: Arc::new(artifact_directory),
        }
    }

    pub(crate) async fn run(
        &self,
        mut command: Command,
        options: &GitRunOptions,
        cancellation: CancellationToken,
    ) -> RuntimeResult<CommandOutput> {
        let started = Instant::now();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut command);

        let artifact = if options.output_mode == GitOutputMode::InlineOrArtifact {
            tokio::fs::create_dir_all(self.artifact_directory.as_ref())
                .await
                .map_err(io_error)?;
            Some(
                self.artifact_directory
                    .join(format!("git-{}.output", uuid::Uuid::new_v4())),
            )
        } else {
            None
        };
        let mut child = command.spawn().map_err(io_error)?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or_else(|| pipe_error("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| pipe_error("stderr"))?;
        let (limit_tx, mut limit_rx) = mpsc::channel(1);
        let stdout_task = tokio::spawn(drain_stream(
            stdout,
            options.max_output_bytes.min(HARD_STDOUT_PREVIEW_BYTES),
            artifact.clone(),
            options.artifact_max_bytes.min(HARD_ARTIFACT_BYTES),
            limit_tx.clone(),
            started,
            "stdout",
        ));
        let stderr_task = tokio::spawn(drain_stream(
            stderr,
            options.max_stderr_bytes.min(HARD_STDERR_PREVIEW_BYTES),
            None,
            0,
            limit_tx,
            started,
            "stderr",
        ));

        let runtime_ms = options
            .timeout_ms
            .min(options.max_runtime_ms)
            .min(HARD_PROCESS_RUNTIME_MS)
            .max(1);
        let deadline = tokio::time::sleep(Duration::from_millis(runtime_ms));
        tokio::pin!(deadline);
        let mut timed_out = false;
        let mut cancelled = false;
        let mut killed_on_limit = false;
        let mut observe_limit = true;
        let status = loop {
            tokio::select! {
                result = child.wait() => break result.map_err(io_error)?,
                () = cancellation.cancelled() => {
                    cancelled = true;
                    terminate_tree(&mut child, pid).await;
                    break child.wait().await.map_err(io_error)?;
                }
                () = &mut deadline => {
                    timed_out = true;
                    terminate_tree(&mut child, pid).await;
                    break child.wait().await.map_err(io_error)?;
                }
                signal = limit_rx.recv(), if observe_limit => {
                    if signal.is_some() && options.kill_on_limit {
                        killed_on_limit = true;
                        terminate_tree(&mut child, pid).await;
                        break child.wait().await.map_err(io_error)?;
                    }
                    observe_limit = false;
                    limit_rx.close();
                }
            }
        };

        let stdout = join_drain(stdout_task, "stdout").await?;
        let stderr = join_drain(stderr_task, "stderr").await?;
        let truncated = stdout.total_bytes > stdout.preview.len() as u64
            || stderr.total_bytes > stderr.preview.len() as u64;
        let artifact_ref = if truncated && stdout.artifact_bytes > 0 {
            artifact.as_ref().map(|path| path.display().to_string())
        } else {
            if let Some(path) = artifact.as_ref() {
                let _ = tokio::fs::remove_file(path).await;
            }
            None
        };
        let truncation_reason = if killed_on_limit {
            Some("outputLimit".to_owned())
        } else if stdout.artifact_limit_reached {
            Some("artifactLimit".to_owned())
        } else if artifact_ref.is_some() {
            Some("contentExternalized".to_owned())
        } else if truncated {
            Some("previewLimit".to_owned())
        } else {
            None
        };

        let artifact_sha256 = artifact_ref.as_ref().map(|_| stdout.sha256);
        Ok(CommandOutput {
            exit_code: status.code(),
            stdout: String::from_utf8_lossy(&stdout.preview).into_owned(),
            stderr: String::from_utf8_lossy(&stderr.preview).into_owned(),
            truncated,
            truncation_reason,
            stdout_bytes: stdout.total_bytes,
            stderr_bytes: stderr.total_bytes,
            artifact_bytes: stdout.artifact_bytes,
            artifact_ref,
            artifact_sha256,
            first_output_ms: [stdout.first_output_ms, stderr.first_output_ms]
                .into_iter()
                .flatten()
                .min(),
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            timed_out,
            cancelled,
            structured: None,
        })
    }
}

struct DrainResult {
    preview: Vec<u8>,
    total_bytes: u64,
    artifact_bytes: u64,
    artifact_limit_reached: bool,
    sha256: String,
    first_output_ms: Option<u64>,
}

async fn drain_stream(
    mut reader: impl AsyncRead + Unpin,
    preview_limit: usize,
    artifact_path: Option<PathBuf>,
    artifact_limit: u64,
    limit_tx: mpsc::Sender<()>,
    started: Instant,
    stream: &'static str,
) -> std::io::Result<DrainResult> {
    let mut preview = Vec::with_capacity(preview_limit.min(READ_CHUNK_BYTES));
    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut total_bytes = 0_u64;
    let mut artifact_bytes = 0_u64;
    let mut artifact_limit_reached = false;
    let mut hasher = Sha256::new();
    let mut first_output_ms = None;
    let mut last_progress_bytes = 0_u64;
    let mut last_progress_at = Instant::now();
    let mut artifact = match artifact_path {
        Some(path) => Some(File::create(path).await?),
        None => None,
    };
    let mut notified = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        first_output_ms.get_or_insert_with(|| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        });
        total_bytes = total_bytes.saturating_add(count as u64);
        if total_bytes.saturating_sub(last_progress_bytes) >= 8 * 1024 * 1024
            || last_progress_at.elapsed() >= Duration::from_secs(1)
        {
            tracing::debug!(
                stream,
                total_bytes,
                artifact_bytes,
                "bounded process output progress"
            );
            last_progress_bytes = total_bytes;
            last_progress_at = Instant::now();
        }
        let preview_remaining = preview_limit.saturating_sub(preview.len());
        preview.extend_from_slice(&buffer[..count.min(preview_remaining)]);
        if let Some(file) = artifact.as_mut() {
            let remaining = artifact_limit.saturating_sub(artifact_bytes);
            let write_count = count.min(usize::try_from(remaining).unwrap_or(usize::MAX));
            if write_count > 0 {
                file.write_all(&buffer[..write_count]).await?;
                hasher.update(&buffer[..write_count]);
                artifact_bytes = artifact_bytes.saturating_add(write_count as u64);
            }
            artifact_limit_reached |= write_count < count;
        }
        if !notified && total_bytes > preview_limit as u64 {
            notified = true;
            let _ = limit_tx.try_send(());
        }
    }
    if let Some(file) = artifact.as_mut() {
        file.flush().await?;
    }
    Ok(DrainResult {
        preview,
        total_bytes,
        artifact_bytes,
        artifact_limit_reached,
        sha256: format!("{:x}", hasher.finalize()),
        first_output_ms,
    })
}

async fn join_drain(
    task: JoinHandle<std::io::Result<DrainResult>>,
    stream: &str,
) -> RuntimeResult<DrainResult> {
    task.await
        .map_err(|error| RuntimeError::new("process_reader_failed", error.to_string()))?
        .map_err(|error| RuntimeError::new("process_reader_failed", format!("{stream}: {error}")))
}

async fn terminate_tree(child: &mut Child, pid: Option<u32>) {
    if let Some(pid) = pid {
        kill_process_tree(pid).await;
    }
    let _ = timeout(Duration::from_secs(2), child.kill()).await;
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(windows)]
async fn kill_process_tree(pid: u32) {
    let _ = Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

#[cfg(unix)]
async fn kill_process_tree(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

#[cfg(not(any(unix, windows)))]
async fn kill_process_tree(_pid: u32) {}

fn pipe_error(stream: &str) -> RuntimeError {
    RuntimeError::new("process_pipe_failed", format!("missing {stream} pipe"))
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("process_start_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn noisy_command(bytes: usize) -> Command {
        if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-Command",
                &format!("$b = New-Object byte[] {bytes}; $o = [Console]::OpenStandardOutput(); $e = [Console]::OpenStandardError(); $o.Write($b, 0, $b.Length); $e.Write($b, 0, $b.Length)"),
            ]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                &format!("head -c {bytes} /dev/zero | tr '\\0' x; head -c {bytes} /dev/zero | tr '\\0' e >&2"),
            ]);
            command
        }
    }

    fn endless_output_command() -> Command {
        if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-Command",
                "$chunk = 'x' * 4096; while ($true) { [Console]::Out.Write($chunk); Start-Sleep -Milliseconds 1 }",
            ]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "while true; do printf '%4096s' x; sleep 0.001; done"]);
            command
        }
    }

    #[tokio::test]
    async fn drains_large_stdout_and_stderr_with_bounded_previews() {
        let directory = TempDir::new().expect("temporary directory");
        let runner = BoundedProcessRunner::new(directory.path().to_owned());
        let options = GitRunOptions {
            max_output_bytes: 4096,
            max_stderr_bytes: 2048,
            artifact_max_bytes: 2 * 1024 * 1024,
            ..GitRunOptions::default()
        };
        let output = runner
            .run(
                noisy_command(1024 * 1024),
                &options,
                CancellationToken::new(),
            )
            .await
            .expect("bounded process output");
        assert_eq!(output.exit_code, Some(0), "output={output:?}");
        assert_eq!(output.stdout.len(), 4096);
        assert_eq!(output.stderr.len(), 2048);
        assert_eq!(output.stdout_bytes, 1024 * 1024);
        assert_eq!(output.stderr_bytes, 1024 * 1024);
        assert_eq!(
            output.truncation_reason.as_deref(),
            Some("contentExternalized")
        );
        assert!(output.artifact_ref.is_some());
    }

    #[tokio::test]
    async fn artifact_limit_caps_disk_bytes_while_draining_to_exit() {
        let directory = TempDir::new().expect("temporary directory");
        let runner = BoundedProcessRunner::new(directory.path().to_owned());
        let options = GitRunOptions {
            max_output_bytes: 1024,
            max_stderr_bytes: 1024,
            artifact_max_bytes: 4096,
            ..GitRunOptions::default()
        };
        let output = runner
            .run(
                noisy_command(128 * 1024),
                &options,
                CancellationToken::new(),
            )
            .await
            .expect("artifact limited output");
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.artifact_bytes, 4096);
        assert_eq!(output.truncation_reason.as_deref(), Some("artifactLimit"));
        assert_eq!(output.stdout_bytes, 128 * 1024);
        assert!(output.artifact_ref.is_some());
    }

    #[tokio::test]
    async fn cancellation_stops_and_reaps_process() {
        let directory = TempDir::new().expect("temporary directory");
        let runner = BoundedProcessRunner::new(directory.path().to_owned());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let command = if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        let output = runner
            .run(command, &GitRunOptions::default(), cancellation)
            .await
            .expect("cancelled output");
        assert!(output.cancelled);
    }

    #[tokio::test]
    async fn timeout_stops_a_hanging_process() {
        let directory = TempDir::new().expect("temporary directory");
        let runner = BoundedProcessRunner::new(directory.path().to_owned());
        let options = GitRunOptions {
            timeout_ms: 50,
            max_runtime_ms: 50,
            ..GitRunOptions::default()
        };
        let command = if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        let output = runner
            .run(command, &options, CancellationToken::new())
            .await
            .expect("timed out output");
        assert!(output.timed_out);
        assert!(output.elapsed_ms < 5_000);
    }

    #[tokio::test]
    async fn kill_on_limit_stops_output_producer() {
        let directory = TempDir::new().expect("temporary directory");
        let runner = BoundedProcessRunner::new(directory.path().to_owned());
        let options = GitRunOptions {
            max_output_bytes: 1024,
            kill_on_limit: true,
            ..GitRunOptions::default()
        };
        let output = runner
            .run(endless_output_command(), &options, CancellationToken::new())
            .await
            .expect("output limited process");
        assert!(output.truncated);
        assert_eq!(output.truncation_reason.as_deref(), Some("outputLimit"));
        assert!(output.elapsed_ms < 5_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_spawned_grandchild_process_group() {
        let directory = TempDir::new().expect("temporary directory");
        let pid_file = directory.path().join("grandchild.pid");
        let runner = BoundedProcessRunner::new(directory.path().join("artifacts"));
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
        ]);
        let options = GitRunOptions {
            timeout_ms: 250,
            max_runtime_ms: 250,
            ..GitRunOptions::default()
        };
        let output = runner
            .run(command, &options, CancellationToken::new())
            .await
            .expect("timed out process tree");
        assert!(output.timed_out);

        let grandchild_pid: u32 = tokio::fs::read_to_string(&pid_file)
            .await
            .expect("grandchild pid file")
            .trim()
            .parse()
            .expect("grandchild pid");
        let status = Command::new("kill")
            .args(["-0", &grandchild_pid.to_string()])
            .status()
            .await
            .expect("probe grandchild");
        assert!(!status.success(), "grandchild survived process-group kill");
    }
}
