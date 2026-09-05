#![allow(clippy::result_large_err)]

use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, GitRunOptions, GitService, PolicyDecision,
    PolicyEngine, RuntimeResult, WorkspaceService,
};
use std::{collections::BTreeMap, path::Path, process::Command, sync::Arc};
use tokio_util::sync::CancellationToken;

struct Approve;
impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn staged_repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp repository");
    git(directory.path(), &["init", "--quiet"]);
    git(
        directory.path(),
        &["config", "user.email", "chatcmd-test@example.invalid"],
    );
    git(directory.path(), &["config", "user.name", "ChatCMD Test"]);
    std::fs::write(directory.path().join("tracked.txt"), "base\n").expect("write base");
    git(directory.path(), &["add", "--all"]);
    git(
        directory.path(),
        &["commit", "--quiet", "--message", "base"],
    );
    std::fs::write(directory.path().join("tracked.txt"), "staged\n").expect("write staged");
    git(directory.path(), &["add", "--", "tracked.txt"]);
    directory
}

fn service(cwd: &Path) -> GitService {
    let workspace = WorkspaceService::new(
        &[cwd.to_path_buf()],
        PolicyEngine::new(
            Some(ExecutionPolicy {
                default: PolicyDecision::Allow,
                per_agent_tool: BTreeMap::new(),
                per_root: BTreeMap::new(),
            }),
            Arc::new(Approve),
        ),
    )
    .expect("create workspace");
    GitService::new(workspace, 64 * 1024)
}

#[cfg(unix)]
fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

async fn assert_failure_preserves_index(directory: &tempfile::TempDir, message: &str) {
    let index_before = git(directory.path(), &["diff", "--cached", "--binary"]);
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);
    let output = service(directory.path())
        .commit_with_options(
            directory.path(),
            message,
            true,
            &[],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("structured git failure");
    assert_ne!(output.exit_code, Some(0));
    assert_eq!(
        git(directory.path(), &["diff", "--cached", "--binary"]),
        index_before
    );
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
}

#[tokio::test]
async fn missing_identity_preserves_the_preexisting_index() {
    let directory = staged_repository();
    git(directory.path(), &["config", "user.name", ""]);
    git(directory.path(), &["config", "user.email", ""]);
    assert_failure_preserves_index(&directory, "identity failure").await;
}

#[tokio::test]
async fn failing_commit_hook_preserves_the_preexisting_index() {
    let directory = staged_repository();
    let hook = directory.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&hook)
            .expect("hook metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).expect("make hook executable");
    }
    assert_failure_preserves_index(&directory, "hook failure").await;
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn symlinked_parent_is_rejected_without_reading_or_committing_outside_content() {
    let directory = staged_repository();
    git(directory.path(), &["reset", "--hard", "HEAD"]);
    std::fs::create_dir(directory.path().join("nested")).expect("create tracked directory");
    std::fs::write(directory.path().join("nested/selected.txt"), "inside\n")
        .expect("write tracked path");
    git(directory.path(), &["add", "--", "nested/selected.txt"]);
    git(
        directory.path(),
        &["commit", "--quiet", "--message", "nested base"],
    );
    std::fs::remove_dir_all(directory.path().join("nested")).expect("remove tracked directory");

    let outside = tempfile::tempdir().expect("outside directory");
    let marker = outside.path().join("selected.txt");
    std::fs::write(&marker, "outside secret marker\n").expect("write outside marker");
    link_directory(outside.path(), &directory.path().join("nested")).expect("link parent outside");
    let head_before = git(directory.path(), &["rev-parse", "HEAD"]);

    let error = service(directory.path())
        .preview_commit_with_options(
            directory.path(),
            false,
            &["nested/selected.txt".to_owned()],
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("symlinked parent must fail closed before returning a digest");

    assert_eq!(error.code, "git_scope_inspection_failed");
    assert!(error.message.contains("symbolic-link"));
    assert_eq!(
        std::fs::read_to_string(&marker).expect("read marker"),
        "outside secret marker\n"
    );
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head_before);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn leaf_symlink_digest_depends_on_link_target_not_target_file_bytes() {
    let directory = staged_repository();
    git(directory.path(), &["reset", "--hard", "HEAD"]);
    let outside = tempfile::NamedTempFile::new().expect("outside file");
    std::fs::write(outside.path(), "first secret\n").expect("write outside content");
    std::fs::remove_file(directory.path().join("tracked.txt")).expect("remove tracked file");
    link_file(outside.path(), &directory.path().join("tracked.txt")).expect("create leaf symlink");
    let paths = ["tracked.txt".to_owned()];
    let first = service(directory.path())
        .preview_commit_with_options(
            directory.path(),
            false,
            &paths,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("first preview");

    std::fs::write(outside.path(), "different outside bytes\n").expect("change outside content");
    let second = service(directory.path())
        .preview_commit_with_options(
            directory.path(),
            false,
            &paths,
            &GitRunOptions::default(),
            CancellationToken::new(),
        )
        .await
        .expect("second preview");

    assert_eq!(first.worktree_digest, second.worktree_digest);
}
