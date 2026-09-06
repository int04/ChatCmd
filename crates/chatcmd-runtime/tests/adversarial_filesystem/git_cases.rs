use super::*;

#[tokio::test]
async fn git_status_spills_large_output_without_exceeding_inline_cap() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("run git init");
    assert!(status.success());
    for index in 0..256 {
        std::fs::write(
            directory
                .path()
                .join(format!("untracked-{index:04}-long-name.txt")),
            b"content\n",
        )
        .expect("write git fixture");
    }
    let service = GitService::new(workspace(directory.path()), 1024);
    let options = GitRunOptions {
        max_output_bytes: 1024,
        max_stderr_bytes: 1024,
        artifact_max_bytes: 1024 * 1024,
        ..GitRunOptions::default()
    };
    let output = service
        .status_with_options(directory.path(), &options, CancellationToken::new())
        .await
        .expect("bounded git status");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.len() <= options.max_output_bytes);
    assert!(output.stdout_bytes > output.stdout.len() as u64);
    assert!(output.truncated);
    assert!(output.artifact_ref.is_some());
    assert!(output.artifact_sha256.is_some());
}

#[tokio::test]
async fn git_commit_success_returns_structured_commit_hash() {
    let directory = tempfile::tempdir().expect("temp directory");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(directory.path())
            .status()
            .expect("configure git fixture");
        assert!(status.success());
    }
    std::fs::write(directory.path().join("tracked.txt"), b"content\n").expect("write fixture");
    let status = Command::new("git")
        .args(["add", "--all"])
        .current_dir(directory.path())
        .status()
        .expect("stage explicit all scope");
    assert!(status.success());
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .commit_with_options(
            directory.path(),
            "successful commit",
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("successful bounded commit");

    assert_eq!(output.exit_code, Some(0), "output={output:?}");
    match output.structured {
        Some(GitStructuredOutput::Commit(data)) => {
            assert_eq!(data.phase, "commitHooksIncluded");
            assert!(data.hooks_included);
            let hash = data.commit_hash.expect("commit hash");
            assert_eq!(hash.len(), 40);
            assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        other => panic!("expected structured commit metadata, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn git_commit_hanging_pre_commit_hook_times_out_and_is_reaped() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temp directory");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(directory.path())
            .status()
            .expect("configure git fixture");
        assert!(status.success());
    }
    std::fs::write(directory.path().join("tracked.txt"), b"content\n").expect("write fixture");
    let status = Command::new("git")
        .args(["add", "--all"])
        .current_dir(directory.path())
        .status()
        .expect("stage explicit all scope");
    assert!(status.success());
    let hook = directory.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, b"#!/bin/sh\nsleep 30\n").expect("write hanging hook");
    let mut permissions = std::fs::metadata(&hook)
        .expect("hook metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).expect("make hook executable");

    let service = GitService::new(workspace(directory.path()), 4096);
    let options = GitRunOptions {
        timeout_ms: 300,
        max_runtime_ms: 300,
        ..GitRunOptions::default()
    };
    let output = service
        .commit_with_options(
            directory.path(),
            "must time out",
            true,
            &[],
            &options,
            CancellationToken::new(),
        )
        .await
        .expect("bounded commit timeout");

    assert!(output.timed_out, "hanging hook must obey git timeout");
    assert!(!output.cancelled);
    assert!(
        output.elapsed_ms < 5_000,
        "hook process tree was not reaped promptly"
    );
    let count = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(directory.path())
        .output()
        .expect("inspect commit result");
    assert!(
        !count.status.success(),
        "timed-out hook must not create a commit"
    );
}

#[tokio::test]
async fn git_corrupt_repository_and_index_lock_fail_without_panicking() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    std::fs::write(directory.path().join(".git/HEAD"), b"not-a-valid-head\n")
        .expect("corrupt HEAD");
    let service = GitService::new(workspace(directory.path()), 4096);
    let status_output = service
        .status_with_options(
            directory.path(),
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("corrupt repository returns process outcome");
    assert_ne!(status_output.exit_code, Some(0));

    let locked = tempfile::tempdir().expect("locked repository");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "chatcmd-test@example.invalid"],
        vec!["config", "user.name", "ChatCMD Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(locked.path())
            .status()
            .expect("configure locked repo");
        assert!(status.success());
    }
    std::fs::write(locked.path().join("tracked.txt"), b"content\n").expect("write fixture");
    let status = Command::new("git")
        .args(["add", "--all"])
        .current_dir(locked.path())
        .status()
        .expect("stage locked fixture");
    assert!(status.success());
    std::fs::write(locked.path().join(".git/index.lock"), b"locked\n").expect("create index lock");
    let service = GitService::new(workspace(locked.path()), 4096);
    let output = service
        .commit_with_options(
            locked.path(),
            "must fail during stage",
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("locked index returns bounded outcome");
    assert_ne!(output.exit_code, Some(0));
    match output.structured {
        Some(GitStructuredOutput::Commit(data)) => assert_eq!(data.phase, "commitHooksIncluded"),
        other => panic!("expected commit failure metadata, got {other:?}"),
    }
}

#[tokio::test]
async fn git_binary_diff_remains_bounded_and_argument_safe() {
    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let binary = (0_u8..=255)
        .cycle()
        .take(2 * 1024 * 1024)
        .collect::<Vec<_>>();
    std::fs::write(directory.path().join("binary.dat"), binary).expect("write binary fixture");
    let status = Command::new("git")
        .args(["add", "--", "binary.dat"])
        .current_dir(directory.path())
        .status()
        .expect("stage binary fixture");
    assert!(status.success());
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .diff_with_options(
            directory.path(),
            true,
            false,
            None,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("bounded binary diff");
    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout_bytes < 64 * 1024);
    assert!(output.stdout.contains("Binary files") || output.stdout.contains("GIT binary patch"));
}

#[cfg(unix)]
#[tokio::test]
async fn git_diff_disables_configured_external_diff() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temp directory");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let marker = directory.path().join("external-diff-ran");
    let script = directory.path().join("external-diff.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\nsleep 30\n", marker.display()),
    )
    .expect("write external diff helper");
    let mut permissions = std::fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make helper executable");
    let status = Command::new("git")
        .args([
            "config",
            "diff.external",
            script.to_str().expect("script path"),
        ])
        .current_dir(directory.path())
        .status()
        .expect("configure external diff");
    assert!(status.success());
    std::fs::write(directory.path().join("file.txt"), b"content\n").expect("write fixture");
    let status = Command::new("git")
        .args(["add", "--", "file.txt"])
        .current_dir(directory.path())
        .status()
        .expect("stage fixture");
    assert!(status.success());
    let service = GitService::new(workspace(directory.path()), 4096);
    let output = service
        .diff_with_options(
            directory.path(),
            true,
            false,
            None,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("diff with external diff disabled");
    assert_eq!(output.exit_code, Some(0));
    assert!(
        !marker.exists(),
        "--no-ext-diff must suppress configured helper"
    );
}
