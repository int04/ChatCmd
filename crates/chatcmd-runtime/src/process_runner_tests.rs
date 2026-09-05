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
            &format!(
                "head -c {bytes} /dev/zero | tr '\\0' x; head -c {bytes} /dev/zero | tr '\\0' e >&2"
            ),
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

fn invalid_utf8_command(bytes: usize) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args([
                "-NoProfile",
                "-Command",
                &format!("$b = [byte[]]::new({bytes}); [Array]::Fill($b, [byte]255); [Console]::OpenStandardOutput().Write($b, 0, $b.Length)"),
            ]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("head -c {bytes} /dev/zero | tr '\\0' '\\377'"),
        ]);
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
async fn invalid_utf8_rendering_cannot_expand_past_preview_budget() {
    let directory = TempDir::new().expect("temporary directory");
    let runner = BoundedProcessRunner::new(directory.path().to_owned());
    let options = GitRunOptions {
        max_output_bytes: 1024,
        output_mode: GitOutputMode::Inline,
        ..GitRunOptions::default()
    };
    let output = runner
        .run(
            invalid_utf8_command(4096),
            &options,
            CancellationToken::new(),
        )
        .await
        .expect("binary output");
    assert_eq!(output.exit_code, Some(0));
    assert!(output.truncated);
    assert!(output.stdout.len() <= 1024);
    assert_eq!(output.stdout_bytes, 4096);
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
